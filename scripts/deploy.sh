#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/deploy.sh <image-tag> [namespace] [release]

Required:
  image-tag   The image tag to deploy (e.g. commit SHA or release version)

Optional:
  namespace   Kubernetes namespace (default: vibe-kanban-team)
  release     Helm release name (default: vibe-kanban)

Env:
  VALUES_FILE Path to values file (default: values-production.yaml)

Examples:
  scripts/deploy.sh 3a088ff6
  VALUES_FILE=values-staging.yaml scripts/deploy.sh 3a088ff6 staging vibe-kanban-staging
USAGE
}

if [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ]; then
  usage
  exit 0
fi

IMAGE_TAG="${1:-}"
if [ -z "${IMAGE_TAG}" ]; then
  usage
  exit 1
fi

NAMESPACE="${2:-vibe-kanban-team}"
RELEASE="${3:-vibe-kanban}"
VALUES_FILE="${VALUES_FILE:-values-production.yaml}"

if ! command -v helm >/dev/null 2>&1; then
  echo "helm not found in PATH"
  exit 1
fi

if [ ! -f "${VALUES_FILE}" ]; then
  echo "Values file not found: ${VALUES_FILE}"
  exit 1
fi

helm upgrade --install "${RELEASE}" ./helm/vibe-kanban-team \
  --namespace "${NAMESPACE}" \
  --create-namespace \
  -f "${VALUES_FILE}" \
  --set "image.tag=${IMAGE_TAG}" \
  --set "relay.image.tag=${IMAGE_TAG}"
