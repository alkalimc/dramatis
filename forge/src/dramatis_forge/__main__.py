"""`python -m dramatis_forge` — the CLI entry point.

Present so the package can be invoked without relying on the installed console script,
which on some systems is defeated by a hidden `.pth` (see the `forge` wrapper).
"""

from .cli import app

if __name__ == "__main__":
    app(prog_name="dramatis-forge")
