FROM python:3.12-slim
WORKDIR /app
COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt
COPY app.py .
# No code changes: the agent wraps the interpreter and reads OTEL_* from
# the environment. The naming standard travels as resource attributes.
ENTRYPOINT ["opentelemetry-instrument"]
CMD ["python", "app.py"]
