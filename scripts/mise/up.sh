#!/usr/bin/env bash
docker compose -f .devcontainer/docker-compose.yml up -d redis postgres localstack nats
echo "Waiting for services..."
until curl -s http://localhost:4566/_localstack/health 2>/dev/null | grep -q '"sqs"'; do sleep 1; done
uvx --from awscli aws sqs create-queue --endpoint-url http://localhost:4566 --queue-name test-queue --region us-east-1 > /dev/null 2>&1 || true
echo "All services ready"
