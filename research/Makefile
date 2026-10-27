# Convenience targets. Set ROOT to the research data directory (units, pairs, eval sets).

PY := .venv/bin/python
CLI := PYTHONPATH=src $(PY) -m dramatis_research
ROOT ?= $(DRAMATIS_RESEARCH_ROOT)

.PHONY: install test lint ablate-plan ablate evaluate

install:
	uv venv --python 3.13
	uv pip install -e '.[lora]' --group dev

test: ; $(PY) -m pytest -p no:cacheprovider
lint: ; $(PY) -m ruff check src tests

ablate-plan: ; $(CLI) ablate plan --root $(ROOT)
ablate: ; $(CLI) ablate run --root $(ROOT) $(ARGS)
evaluate: ; $(CLI) evaluate --root $(ROOT) $(ARGS)
