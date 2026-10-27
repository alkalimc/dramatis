"""Fine-tuning: an embedding model (contrastive) and a yes/no reranker (listwise).

Both trainers take their data from `data.select_training_pairs`, so the leak check, the
ablation flags and the data-size limit behave identically for the two. Every run writes
`<runs>/<run-id>/run.json` with the config, hashes of the input files, the selection digest,
timing and the cost figure `train.gpu_hours`.
"""

from __future__ import annotations

import json
import platform
import random
import time
from dataclasses import asdict, dataclass, field
from datetime import UTC, datetime
from pathlib import Path

from . import data, models
from .encoders import YesNoReranker, is_accelerator, pick_device
from .windows import Windowing, build_windows

RUN_JSON = "run.json"
MODEL_DIR = "model"


@dataclass
class TrainConfig:
    root: Path
    kind: str = "embed"  # "embed" | "rerank"
    base: str = ""
    pairs: str = data.STRUCTURAL
    negatives: str = data.STRUCTURAL
    limit: int = 0
    seed: int = 0
    drop_families: list[str] = field(default_factory=list)
    window: int = 0  # 0 = typed units; N = fixed windows of N characters
    hard_negatives: int = 1  # negatives per example (embed) / per group (rerank)
    epochs: int = 1
    batch_size: int = 16
    learning_rate: float = 2e-5
    max_seq_length: int = 256
    query_instruction: str = models.QUERY_INSTRUCTION
    lora_rank: int = 0  # 0 = full fine-tuning
    bf16: bool = False
    gradient_checkpointing: bool = False
    device: str = ""
    run_id: str = ""
    runs_dir: Path | None = None

    def resolved_base(self) -> str:
        return self.base or (models.EMBED_MODEL if self.kind == "embed" else models.RERANK_MODEL)

    def resolved_runs_dir(self) -> Path:
        return self.runs_dir or (self.root / "runs")


def make_run_id(cfg: TrainConfig) -> str:
    if cfg.run_id:
        return cfg.run_id
    parts = [cfg.kind, cfg.pairs, f"neg-{cfg.negatives}", f"n{cfg.limit or 'all'}", f"s{cfg.seed}"]
    if cfg.drop_families:
        parts.append("drop-" + "+".join(sorted(cfg.drop_families)))
    if cfg.window:
        parts.append(f"win{cfg.window}")
    return "_".join(parts)


def prepare(cfg: TrainConfig) -> tuple[data.Selection, Windowing | None]:
    units = data.load_units(cfg.root)
    windowing = build_windows(units, cfg.window) if cfg.window else None
    selection = data.select_training_pairs(
        cfg.root,
        units,
        pairs=cfg.pairs,
        negatives=cfg.negatives,
        limit=cfg.limit,
        seed=cfg.seed,
        drop_families=cfg.drop_families,
        neg_count=cfg.hard_negatives,
        windowing=windowing,
    )
    if not selection.examples:
        raise ValueError("no training pairs selected")
    return selection, windowing


def _input_hashes(cfg: TrainConfig) -> dict[str, str]:
    names = [data.UNITS, data.PAIRS_TRAIN, data.SYNTH_TRAIN, data.PAIRS_TEST, data.SYNTH_TEST]
    return {n: data.sha256_file(cfg.root / n) for n in names if (cfg.root / n).exists()}


def _llm_calls(cfg: TrainConfig) -> int | None:
    """LLM calls spent producing this run's training data (structural pairs cost none)."""
    if cfg.pairs == data.STRUCTURAL:
        return 0
    cost = cfg.root / data.COST
    if cost.exists():
        return json.loads(cost.read_text()).get("synthetic_llm_calls")
    return None


def write_run_json(cfg: TrainConfig, selection: data.Selection, run_dir: Path, **extra) -> dict:
    import torch
    import transformers

    train_seconds = extra.pop("train_seconds")
    device = extra.pop("device")
    accel = is_accelerator(device)
    info = {
        "run_id": run_dir.name,
        "kind": cfg.kind,
        "created": datetime.now(UTC).isoformat(timespec="seconds"),
        "config": {k: (str(v) if isinstance(v, Path) else v) for k, v in asdict(cfg).items()},
        "base_model": cfg.resolved_base(),
        "model_dir": MODEL_DIR,
        "data": {
            "inputs": _input_hashes(cfg),
            "selection_digest": selection.digest,
            "examples": len(selection.examples),
            "by_family": selection.counts,
            "llm_calls_for_data": _llm_calls(cfg),
        },
        "time": {"train_seconds": train_seconds, "wall_hours": train_seconds / 3600},
        "device": device,
        # Hours on an accelerator (cuda or MPS); CPU time is reported separately so the cost
        # column never hides which hardware produced it.
        "train.gpu_hours": train_seconds / 3600 if accel else 0.0,
        "train.cpu_hours": 0.0 if accel else train_seconds / 3600,
        "versions": {
            "python": platform.python_version(),
            "torch": torch.__version__,
            "transformers": transformers.__version__,
        },
        **extra,
    }
    (run_dir / RUN_JSON).write_text(json.dumps(info, indent=2, sort_keys=True) + "\n")
    return info


def load_run(runs_dir: Path, run_id: str) -> dict:
    return json.loads((runs_dir / run_id / RUN_JSON).read_text())


def _prepare_run_dir(cfg: TrainConfig) -> Path:
    run_dir = cfg.resolved_runs_dir() / make_run_id(cfg)
    run_dir.mkdir(parents=True, exist_ok=True)
    return run_dir


def embed_datasets(cfg: TrainConfig, selection: data.Selection) -> dict:
    """Examples grouped by how many hard negatives they carry, one dataset per group, because
    the contrastive loss needs a fixed column layout per dataset (synthetic pairs have none)."""
    from datasets import Dataset

    groups: dict[int, list[data.TrainExample]] = {}
    for ex in selection.examples:
        groups.setdefault(min(len(ex.negatives), cfg.hard_negatives), []).append(ex)
    out = {}
    for n, items in sorted(groups.items()):
        cols = {
            "anchor": [models.format_query(cfg.query_instruction, e.query) for e in items],
            "positive": [e.positive for e in items],
        }
        for j in range(n):
            cols[f"negative_{j + 1}"] = [e.negatives[j] for e in items]
        out[f"neg{n}"] = Dataset.from_dict(cols)
    return out


def train_embed(cfg: TrainConfig) -> Path:
    from sentence_transformers import (
        SentenceTransformer,
        SentenceTransformerTrainer,
        SentenceTransformerTrainingArguments,
    )
    from sentence_transformers.base.sampler import BatchSamplers
    from sentence_transformers.sentence_transformer.losses import MultipleNegativesRankingLoss

    selection, _ = prepare(cfg)
    run_dir = _prepare_run_dir(cfg)
    device = cfg.device or pick_device()
    model = SentenceTransformer(cfg.resolved_base(), device=device)
    model.max_seq_length = cfg.max_seq_length
    if cfg.lora_rank:
        from peft import LoraConfig

        model.add_adapter(LoraConfig(r=cfg.lora_rank, lora_alpha=2 * cfg.lora_rank))

    datasets = embed_datasets(cfg, selection)
    args = SentenceTransformerTrainingArguments(
        output_dir=str(run_dir / "checkpoints"),
        num_train_epochs=cfg.epochs,
        per_device_train_batch_size=cfg.batch_size,
        learning_rate=cfg.learning_rate,
        warmup_ratio=0.1,
        seed=cfg.seed,
        data_seed=cfg.seed,
        # In-batch negatives are only valid when no two rows share a text.
        batch_sampler=BatchSamplers.NO_DUPLICATES,
        bf16=cfg.bf16,
        gradient_checkpointing=cfg.gradient_checkpointing,
        use_cpu=(device == "cpu"),
        save_strategy="no",
        logging_steps=10,
        report_to="none",
        disable_tqdm=True,
    )
    trainer = SentenceTransformerTrainer(
        model=model,
        args=args,
        train_dataset=datasets,
        loss=MultipleNegativesRankingLoss(model),
    )
    started = time.perf_counter()
    trainer.train()
    seconds = time.perf_counter() - started
    model.save(str(run_dir / MODEL_DIR))
    write_run_json(cfg, selection, run_dir, train_seconds=seconds, device=device)
    return run_dir


def rerank_groups(
    cfg: TrainConfig, selection: data.Selection, rng: random.Random
) -> list[list[tuple[str, str]]]:
    """One group per example: the positive pair first, then its negatives. Examples without
    a negative cannot teach a ranking and are skipped (synthetic pairs under structural
    negatives)."""
    groups = []
    for ex in selection.examples:
        if not ex.negatives:
            continue
        docs = [ex.positive, *ex.negatives[: cfg.hard_negatives]]
        groups.append([(ex.query, d) for d in docs])
    rng.shuffle(groups)
    return groups


def train_rerank(cfg: TrainConfig, reranker: YesNoReranker | None = None) -> Path:
    import torch

    selection, _ = prepare(cfg)
    run_dir = _prepare_run_dir(cfg)
    device = cfg.device or pick_device()
    torch.manual_seed(cfg.seed)
    rng = random.Random(cfg.seed)
    dtype = torch.bfloat16 if cfg.bf16 else None
    rr = reranker or YesNoReranker(
        cfg.resolved_base(),
        device=device,
        max_length=cfg.max_seq_length,
        instruction=models.RERANK_INSTRUCTION,
        dtype=dtype,
    )
    if cfg.lora_rank:
        from peft import LoraConfig, get_peft_model

        rr.model = get_peft_model(
            rr.model, LoraConfig(r=cfg.lora_rank, lora_alpha=2 * cfg.lora_rank)
        )
    if cfg.gradient_checkpointing:
        rr.model.gradient_checkpointing_enable()
        rr.model.enable_input_require_grads()

    params = [p for p in rr.model.parameters() if p.requires_grad]
    opt = torch.optim.AdamW(params, lr=cfg.learning_rate)
    started = time.perf_counter()
    rr.model.train()
    log: list[float] = []
    for _epoch in range(cfg.epochs):
        groups = rerank_groups(cfg, selection, rng)
        if not groups:
            raise ValueError("no pair has a negative; the reranker needs hard or random negatives")
        for i in range(0, len(groups), cfg.batch_size):
            batch = groups[i : i + cfg.batch_size]
            loss = torch.zeros((), device=rr.device)
            for group in batch:
                margin = rr.margin_logits(group).float()
                # Listwise: the positive (index 0) must beat its negatives.
                loss = loss + torch.nn.functional.cross_entropy(
                    margin.unsqueeze(0), torch.zeros(1, dtype=torch.long, device=margin.device)
                )
            loss = loss / len(batch)
            opt.zero_grad()
            loss.backward()
            opt.step()
            log.append(float(loss.detach().cpu()))
    seconds = time.perf_counter() - started
    rr.model.eval()
    out = run_dir / MODEL_DIR
    if cfg.lora_rank:
        rr.model = rr.model.merge_and_unload()
    rr.model.save_pretrained(out)
    rr.tok.save_pretrained(out)
    write_run_json(
        cfg,
        selection,
        run_dir,
        train_seconds=seconds,
        device=device,
        losses={"first": log[0], "last": log[-1], "steps": len(log)},
    )
    return run_dir
