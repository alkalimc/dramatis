# Convenience targets. Everything here is a one-liner you could type yourself; the
# point is that `install` carries a macOS workaround that is easy to lose.

VENV := .venv
PY   := $(VENV)/bin/python
CLI  := ./forge

.PHONY: install test lint sync corpus evals report all clean

install:
	uv venv --python 3.13
	uv pip install -e '.[zh]' --group dev
	@# Use ./forge rather than the installed console script. On this machine every .pth
	@# written into site-packages gets flagged hidden, and CPython's `site` skips hidden
	@# path files — so an editable install succeeds and is then invisible to imports.
	@# The wrapper sets PYTHONPATH, which that quirk cannot reach.
	@./forge --help > /dev/null && echo "install ok — use ./forge (or make sync/corpus/...)"

test:
	$(PY) -m pytest -q

# Full pipeline, in order. Each stage is idempotent, so re-running is cheap.
sync:    ; $(CLI) harvest sync
corpus:  ; $(CLI) corpus build
evals:   ; $(CLI) evals build
report:  ; $(CLI) report coverage && $(CLI) report figures

all: corpus evals report

clean:
	find . -name __pycache__ -type d -prune -exec rm -rf {} +
	rm -rf .pytest_cache
