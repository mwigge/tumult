"""``python -m forecastd``: serve on ``FORECASTD_BIND`` (default
``0.0.0.0:8700``) with uvicorn when FastAPI is installed, else with the
stdlib server. The model loads once, before the socket opens."""

from __future__ import annotations

import logging
import os
import sys

from .model import load_model_from_env


def parse_bind(bind: str) -> tuple[str, int]:
    host, _, port = bind.rpartition(":")
    if not host or not port.isdigit():
        raise ValueError(f"FORECASTD_BIND must be host:port, got {bind!r}")
    return host, int(port)


def main() -> int:
    logging.basicConfig(
        level=os.environ.get("FORECASTD_LOG_LEVEL", "INFO").upper(),
        format="%(asctime)s %(levelname)s %(name)s %(message)s",
    )
    log = logging.getLogger("forecastd")
    host, port = parse_bind(os.environ.get("FORECASTD_BIND", "0.0.0.0:8700"))
    model = load_model_from_env()
    log.info("loaded model %s", model.info.as_dict())
    try:
        import uvicorn

        from .asgi import create_app
    except ImportError:
        from .server import serve

        log.info("fastapi/uvicorn not installed; serving with the stdlib server on %s:%d", host, port)
        server = serve(model, host, port)
        try:
            server.serve_forever()
        except KeyboardInterrupt:
            pass
        finally:
            server.server_close()
        return 0
    uvicorn.run(create_app(model), host=host, port=port, log_level="info")
    return 0


if __name__ == "__main__":
    sys.exit(main())
