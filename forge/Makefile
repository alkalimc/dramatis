# Convenience targets. `install` carries a macOS workaround that is easy to lose; the rest
# are the two commands you would type anyway.

CLI := ./forge

.PHONY: install sync build clean

install:
	uv venv --python 3.13
	uv pip install -e '.[zh,tokens]'
	@# Use ./forge rather than the installed console script. On some machines every .pth
	@# written into site-packages is flagged hidden and CPython's `site` skips it, so an
	@# editable install succeeds and is then invisible to imports. The wrapper sets
	@# PYTHONPATH, which that quirk cannot reach.
	@$(CLI) --help > /dev/null && echo "install ok — use ./forge (or make sync / make build)"

sync:  ; $(CLI) sync
build: ; $(CLI) build

clean:
	find . -name __pycache__ -type d -prune -exec rm -rf {} +
