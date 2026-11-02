"""Package entry point so python -m deadman runs the command line."""

from .cli import main

if __name__ == "__main__":
    raise SystemExit(main())
