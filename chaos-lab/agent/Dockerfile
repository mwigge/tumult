FROM python:3.12-slim@sha256:05cda9777409a9c3ffddd94a4c476b79f0769a0b4857f0c7ed9226b6800b0d6f AS binary
COPY scripts/install_tumult.py /tmp/install_tumult.py
RUN python /tmp/install_tumult.py && tumult --version

FROM python:3.12-slim@sha256:05cda9777409a9c3ffddd94a4c476b79f0769a0b4857f0c7ed9226b6800b0d6f
ENV PYTHONDONTWRITEBYTECODE=1 PYTHONUNBUFFERED=1 HOME=/tmp
WORKDIR /app
COPY agent/requirements.txt /app/requirements.txt
RUN pip install --no-cache-dir -r requirements.txt && mkdir /data && chown 10001:10001 /data
COPY --from=binary /usr/local/bin/tumult /usr/local/bin/tumult
COPY --from=binary /usr/local/bin/tumult-net-proxyd /usr/local/bin/tumult-net-proxyd
COPY agent/ /app/agent/
COPY web/ /app/web/
USER 10001:10001
EXPOSE 8000
CMD ["python", "-m", "uvicorn", "agent.app:app", "--host", "0.0.0.0", "--port", "8000", "--no-access-log", "--no-proxy-headers"]
