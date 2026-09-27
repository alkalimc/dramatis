# Convenience targets. `install` carries a macOS workaround that is easy to lose; the rest
# are the two commands you would type anyway.

CLI := ./forge

PY := .venv/bin/python
CARGO ?= cargo
#: Where `contract` writes the toy folio the engine's contract tests open.
CONTRACT_FOLIO := $(CURDIR)/.contract/toy.folio

.PHONY: install sync build test lint contract clean

install:
	uv venv --python 3.13
	uv pip install -e '.[zh,tokens]' --group dev
	@# Use ./forge rather than the installed console script. On some machines every .pth
	@# written into site-packages is flagged hidden and CPython's `site` skips it, so an
	@# editable install succeeds and is then invisible to imports. The wrapper sets
	@# PYTHONPATH, which that quirk cannot reach.
	@$(CLI) --help > /dev/null && echo "install ok — use ./forge (or make sync / make build)"

sync:  ; $(CLI) sync
build: ; $(CLI) build

test: ; $(PY) -m pytest -p no:cacheprovider
lint: ; $(PY) -m ruff check --no-cache src tests

# Forge → engine contract: build the toy test pack's folio, then open and search it from Rust.
contract:
	DRAMATIS_CONTRACT_OUT=$(CONTRACT_FOLIO) $(PY) -m pytest -p no:cacheprovider \
		tests/test_folio.py::test_contract_folio
	cd ../engine && DRAMATIS_CONTRACT_FOLIO=$(CONTRACT_FOLIO) DRAMATIS_REQUIRE_CONTRACT=1 \
		$(CARGO) test -p folio -p index --test contract

clean:
	rm -rf .contract
	find . -name __pycache__ -type d -prune -exec rm -rf {} +
