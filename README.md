# Vibe Kanban Team

Open source team deployment and release layer for [Vibe Kanban](https://github.com/BloopAI/vibe-kanban).

Vibe Kanban itself already supports team collaboration through a shared remote server. `Vibe Kanban Team` is the name of this distribution because it goes beyond the standard "each developer runs their own frontend installation" model and packages a team-ready, multi-user frontend setup on top of the upstream system. This repo bundles the upstream app, the downstream patch stack, the public Helm chart, and the release automation needed to run that shared installation.

## Overview

This repository provides a deployment and integration layer for Vibe Kanban with:

- **Helm Chart**: Deploys Vibe Kanban remote server, optional relay server, and ElectricSQL
- **Multi-User Frontend Runtime**: Adds a shared frontend/workspace model for simultaneous browser-based use
- **Linear Patch Stack**: One ordered downstream patch series applied to every build
- **Environment-Agnostic Images**: Build once, deploy anywhere
- **External Database**: Bring your own PostgreSQL (CloudNativePG, RDS, etc.)

## What This Adds

The upstream architecture already allows many frontend installations to connect to one central remote server. `Vibe Kanban Team` keeps that central shared backend model and adds a different frontend operating model:

- a central installation that developers can open directly in the browser without installing the stack locally
- team-ready frontend workspaces that can be used by multiple developers at the same time
- shared environments that make pair AI engineering and collaborative debugging practical

In other words, this repo is not claiming that upstream Vibe Kanban is not collaborative. It is packaging a stronger shared-workspace mode for teams that want a centrally managed setup.

## Downstream Feature Snapshot

Current upstream base: `v0.1.44-20260424091429`.

This distribution currently carries 27 downstream patches across 13 main feature and stability areas:

1. Helm-packaged Remote, Relay, ElectricSQL, and optional browser frontend deployment.
2. Shared browser-first frontend runtime with code-server and reusable workspace environments.
3. Workspace auth, browser-scoped sessions, and owner-aware standalone workspace handling.
4. Zoho OAuth support plus optional allowed-email-domain restrictions.
5. Kimi Code executor support plus refreshed stable CLI pins for Claude Code, Codex, Gemini, Qwen, Copilot, and OpenCode, with Codex `npx` cache isolation for shared-cache workspaces.
6. GitLab merge request integration alongside existing GitHub flows.
7. Markdown preview controls in workspace change review.
8. Browser notifications for workspace and execution events.
9. R2-backed attachment storage for the remote service.
10. `VSCODE_PROXY_URI`, localhost link rewriting, and preview proxy support for managed frontend pods.
11. Release and deployment automation for npm, images, relay, and Helm chart publishing.
12. Operational stability fixes for relay builds, WebSocket keepalives, org selection, editor onboarding, and cloud UI behavior.
13. Project kanban restoration for self-hosted cloud deployments.

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                        Kubernetes Cluster                        │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  ┌─────────────────┐    ┌─────────────────┐                     │
│  │                 │    │                 │                     │
│  │  Vibe Kanban    │───▶│   ElectricSQL   │──┐                  │
│  │  Remote Server  │    │   (Sync Layer)  │  │                  │
│  │                 │    │                 │  │                  │
│  │  Port: 8081     │    │  Port: 3000     │  │                  │
│  └────────┬────────┘    └─────────────────┘  │                  │
│           │                                   │                  │
│  ┌────────▼────────┐                         │                  │
│  │     Ingress     │                         │                  │
│  └────────┬────────┘                         │                  │
│           │                                   │                  │
└───────────┼───────────────────────────────────┼──────────────────┘
            │                                   │
            ▼                                   ▼
        Internet                     ┌──────────────────┐
                                     │    PostgreSQL    │
                                     │  (External DB)   │
                                     │  CloudNativePG   │
                                     │  RDS / etc.      │
                                     └──────────────────┘
```

## Prerequisites

- Kubernetes cluster (1.24+)
- Helm 3.x
- PostgreSQL 14+ with `wal_level=logical` (CloudNativePG recommended)
- kubectl configured to access your cluster
- cert-manager installed via the upstream Helm chart (do not use the MicroK8s cert-manager addon)

## cert-manager Installation (Helm, Recommended)

TLS in this chart relies on cert-manager. Install cert-manager using the upstream Helm chart so you stay on a supported release line.

For MicroK8s users, enable core addons without cert-manager:

```bash
microk8s enable dns ingress hostpath-storage community cloudnative-pg
# Intentionally skip: microk8s enable cert-manager
```

Install cert-manager:

```bash
CERT_MANAGER_CHART_VERSION="1.19.4" # update to latest supported patch release

helm repo add jetstack https://charts.jetstack.io
helm repo update

kubectl create namespace cert-manager --dry-run=client -o yaml | kubectl apply -f -

helm upgrade --install cert-manager jetstack/cert-manager \
  --namespace cert-manager \
  --version "${CERT_MANAGER_CHART_VERSION}" \
  --set crds.enabled=true

kubectl -n cert-manager rollout status deploy/cert-manager --timeout=180s
kubectl -n cert-manager rollout status deploy/cert-manager-webhook --timeout=180s
kubectl -n cert-manager rollout status deploy/cert-manager-cainjector --timeout=180s
```

Create a ClusterIssuer (Cloudflare DNS-01 example, supports wildcard code-server port proxy hosts):

```bash
kubectl -n cert-manager create secret generic cloudflare-dns-api-token \
  --from-literal=API_TOKEN='<cloudflare-api-token>'

kubectl apply -f - <<'EOF'
apiVersion: cert-manager.io/v1
kind: ClusterIssuer
metadata:
  name: cert-manager-global
spec:
  acme:
    email: you@example.com
    server: https://acme-v02.api.letsencrypt.org/directory
    privateKeySecretRef:
      name: cert-manager-global-account-key
    solvers:
      - dns01:
          cloudflare:
            apiTokenSecretRef:
              name: cloudflare-dns-api-token
              key: API_TOKEN
EOF
```

Cloudflare token minimum permissions:
- `Zone:Read`
- `DNS:Edit`

## Installation

Install from the published OCI Helm chart:

```bash
export CHART_REF="oci://ghcr.io/iamriajul/helm-charts/vibe-kanban-team"
export CHART_VERSION="<version>"
```

If you prefer GitOps from source, you can still reference the chart in this repository. For normal installs, the OCI chart is the primary path.

## Quick Start

### 1. Prepare PostgreSQL Database

Your PostgreSQL must have logical replication enabled:

```sql
-- CloudNativePG has wal_level=logical by default
-- For other providers, ensure wal_level=logical in postgresql.conf

-- Create the electric_sync role for ElectricSQL
CREATE ROLE electric_sync WITH LOGIN PASSWORD 'your-electric-password' REPLICATION;
GRANT ALL PRIVILEGES ON DATABASE your_database TO electric_sync;
```

If you use the CNPG manifests in `k8s/cnpg/`, the `electric_sync` role is created and granted automatically via the init SQL secret. Keep the ElectricSQL password in sync with the value in `k8s/cnpg/02-initdb-secret.yaml`.

### 2. Create Namespace and Kubernetes Secrets

```bash
kubectl create namespace vibe-kanban-team

# Database connection URLs
kubectl create secret generic vibe-kanban-db \
  --namespace vibe-kanban-team \
  --from-literal=url='postgres://user:pass@your-db-host:5432/remote' \
  --from-literal=electric-url='postgresql://electric_sync:pass@your-db-host:5432/remote?sslmode=disable'

# Application secrets
kubectl create secret generic vibe-kanban-secrets \
  --namespace vibe-kanban-team \
  --from-literal=jwt-secret="$(openssl rand -base64 32)" \
  --from-literal=electric-role-password='your-electric-password'

# OAuth credentials
kubectl create secret generic vibe-kanban-oauth \
  --namespace vibe-kanban-team \
  --from-literal=github-client-id='your-github-client-id' \
  --from-literal=github-client-secret='your-github-client-secret'
```

### 3. (If Needed) Create Image Pull Secret

If your image registry is private, create a pull secret and reference it in `imagePullSecrets`:

```bash
kubectl create secret docker-registry registry-credentials \
  --namespace vibe-kanban-team \
  --docker-server='your-registry.example.com' \
  --docker-username='your-registry-username' \
  --docker-password='your-registry-token' \
  --docker-email='your-email@example.com'
```

### 4. Create Values File

```bash
curl -fsSL \
  https://raw.githubusercontent.com/iamriajul/vibe-kanban-team/main/helm/vibe-kanban-team/values-example.yaml \
  -o values-production.yaml
# Edit values-production.yaml with your secret names and image repositories.
```

Set `global.domain` to the exact frontend hostname users should open, for example `vk.example.com`.
For a full frontend install with remote, relay, and code-server port proxying, configure two DNS records to your ingress controller:

```text
vk.example.com   -> ingress
*.vk.example.com -> ingress
```

The chart derives service hosts from that domain:

```text
frontend:    vk.example.com
remote API:  remote.vk.example.com
relay:       relay.vk.example.com
code-server: code.vk.example.com
port proxy:  <port>-code.vk.example.com
```

The wildcard is for derived service subdomains and code-server port proxying. Relay uses path-based routing on `relay.<domain>` and does not need `*.relay.<domain>`.

code-server runs with its own auth disabled because ingress auth owns access control. If `frontend.codeServerIngress.enabled=true`, the chart now requires either:

- `frontend.auth.enabled=true` with a supported ingress configuration
- `frontend.codeServerIngress.allowUnauthenticated=true` when another layer already protects the ingress

For nginx, auth annotations are derived automatically when `global.ingressClassName` contains `nginx`. For Traefik, set `frontend.auth.createTraefikMiddleware=true` with a Traefik ingress class, or provide `frontend.auth.protectedIngressAnnotations`.

The frontend app can be preconfigured through env vars, including Coder workspace injection:

- `VIBE_KANBAN_EDITOR_TYPE=CODE_SERVER`
- `VIBE_KANBAN_CODE_SERVER_URL=https://code.vk.example.com/` (`CODE_SERVER_URL` also works)
- `VIBE_KANBAN_BYPASS_ONBOARDING=true`

### 5. Deploy

```bash
helm upgrade --install vibe-kanban "${CHART_REF}" \
  --version "${CHART_VERSION}" \
  --namespace vibe-kanban-team \
  --create-namespace \
  -f values-production.yaml
```

If you want the chart's complete raw defaults for reference, you can still inspect them with:

```bash
helm show values "${CHART_REF}" --version "${CHART_VERSION}"
```

If you want to pin a specific image tag, use:

```bash
scripts/deploy.sh <commit-sha>
```

## Configuration

This chart follows the same pattern as the [Coder Helm chart](https://coder.com/docs/install/kubernetes): reference your own Kubernetes secrets via `secretKeyRef`.

### Example values.yaml

```yaml
env:
  # Database connection (REQUIRED)
  - name: SERVER_DATABASE_URL
    valueFrom:
      secretKeyRef:
        name: vibe-kanban-db
        key: url

  # JWT secret (REQUIRED)
  - name: VIBEKANBAN_REMOTE_JWT_SECRET
    valueFrom:
      secretKeyRef:
        name: vibe-kanban-secrets
        key: jwt-secret

  # ElectricSQL role password (REQUIRED)
  - name: ELECTRIC_ROLE_PASSWORD
    valueFrom:
      secretKeyRef:
        name: vibe-kanban-secrets
        key: electric-role-password

  # GitHub OAuth (REQUIRED - at least one OAuth provider)
  - name: GITHUB_OAUTH_CLIENT_ID
    valueFrom:
      secretKeyRef:
        name: vibe-kanban-oauth
        key: github-client-id
  - name: GITHUB_OAUTH_CLIENT_SECRET
    valueFrom:
      secretKeyRef:
        name: vibe-kanban-oauth
        key: github-client-secret

# ElectricSQL database connection
electric:
  enabled: true
  env:
    - name: DATABASE_URL
      valueFrom:
        secretKeyRef:
          name: vibe-kanban-db
          key: electric-url
```

### Required Environment Variables

| Variable | Description |
|----------|-------------|
| `SERVER_DATABASE_URL` | PostgreSQL connection URL |
| `VIBEKANBAN_REMOTE_JWT_SECRET` | JWT secret (generate with `openssl rand -base64 32`) |
| `ELECTRIC_ROLE_PASSWORD` | Password for the `electric_sync` database role |
| `GITHUB_OAUTH_CLIENT_ID` | GitHub OAuth client ID |
| `GITHUB_OAUTH_CLIENT_SECRET` | GitHub OAuth client secret |

If you're using the CNPG manifests, set `ELECTRIC_ROLE_PASSWORD` to the same value as `CHANGEME_ELECTRIC_PASSWORD` in `k8s/cnpg/02-initdb-secret.yaml`.

### Optional: Relay/Tunnel Deployment

To support tunnel/relay features, enable the `relay` section in values and configure:

- `relay.enabled: true`
- `relay.env` with `SERVER_DATABASE_URL` and `VIBEKANBAN_REMOTE_JWT_SECRET` (same DB/JWT as remote)
- `relay.ingress` or `global.domain` so the chart exposes one relay host (for example `relay.example.com`)
- keep `relay.proxyUnderRemoteIngress.enabled: true` so relay endpoints are available under the main remote API host (`/v1/relay` and `/relay/h`) for reusable frontend images

`scripts/deploy.sh` now sets both `image.tag` and `relay.image.tag` to the requested release tag.

### Database Requirements

Your PostgreSQL database must have:

1. **Logical replication enabled**: `wal_level=logical`
   - CloudNativePG: Enabled by default
   - Other providers: Set in `postgresql.conf`

2. **ElectricSQL role**: User with `REPLICATION` privilege
   ```sql
   CREATE ROLE electric_sync WITH LOGIN PASSWORD 'xxx' REPLICATION;
   ```

If you use the CNPG manifests, the role is created by the init SQL secret.

### OAuth Setup

#### GitHub OAuth

1. Go to GitHub → Settings → Developer settings → OAuth Apps
2. Create new OAuth App:
   - Homepage URL: `https://your-domain.com`
   - Callback URL: `https://your-domain.com/v1/oauth/callback/github`
3. Copy Client ID and Client Secret

#### Google OAuth

1. Go to Google Cloud Console → APIs & Services → Credentials
2. Create OAuth 2.0 Client ID:
   - Application type: Web application
   - Authorized redirect URIs: `https://your-domain.com/v1/oauth/callback/google`
3. Copy Client ID and Client Secret

## Release Automation

GitHub Actions now handle the checked-in release flows:
- `remote-v*` tags build Remote and Relay images, push to GHCR, optionally mirror to Docker Hub, and publish the Helm chart to GHCR as an OCI artifact
- `v*` tags publish the `vibe-kanban-team` npm package through `scripts/publish-npm.sh`

For stable releases, the image workflow also updates the `latest` tag. Prereleases publish only their version tag.

## Why "Team"

The upstream project is still Vibe Kanban. `Vibe Kanban Team` names this public distribution layer:

- it keeps the upstream shared remote server model, but adds a team-ready frontend/workspace deployment shape
- it lets developers use the platform without installing the full stack on their own machines
- it lets developers share live workspaces for pair AI engineering, review, and debugging
- it gives teams a reproducible central environment instead of many drifting local installs
- it shortens onboarding because a new developer can start from a browser session instead of a full local setup
- it makes support, upgrades, and operational policy easier because the environment is managed centrally
- it publishes the npm entrypoint, container images, and Helm chart under one public name

The goal is to make the “run Vibe Kanban for a team with shared workspaces” path obvious and easy to adopt.

## Release Tracking (Upstream Vibe Kanban)

We track upstream releases from the Vibe Kanban GitHub repo and bump the shared `vibe-kanban/` submodule when we want a new feature or fix. Keep it simple:

1. Watch for new upstream releases (GitHub Releases/notifications).
2. Decide the version to adopt (e.g. `v1.4.0`).
3. Update the shared submodule and patch stack.
4. Push the release tag that matches the artifact flow you want.
5. Deploy by pinning the image tag.

## Patch Stack (Downstream Changes)

We keep downstream changes as a small patch stack in `patches/` (similar to quilt). The local scripts and GitHub Actions both apply this same stack before building.

### Creating a Patch

```bash
cd vibe-kanban
git checkout <upstream-tag>
# Make your change(s)
git add .
git commit -m "fix: <summary>"
git format-patch -1 -o ../patches
cd ..
ls patches
```

Rename the patch into the next `NNNN-...patch` slot and add it to `patches/series` in the order you want it applied.

### Applying Patches Locally

```bash
scripts/apply-patches.sh
```

Keep the patch stack minimal and prefer upstreaming when possible.

## Upgrading Vibe Kanban (Process)

```bash
# 1) Update the shared submodule to a tag or commit
scripts/update-vibe-kanban.sh v1.4.0

# 2) Review and commit
git status
git commit -m "chore: bump vibe-kanban to v1.4.0"
git push
```

After merge, CI builds and pushes a new image tagged with the commit SHA.

```bash
# 3) Deploy the new build by pinning the image tag
scripts/deploy.sh <commit-sha>
```

If you want a versioned release tag for this repo (optional), create a tag like `v1.4.0` and push it. CI will also publish a release image tag and a chart package.

## Troubleshooting

### Check Pod Status

```bash
kubectl get pods -n vibe-kanban-team
kubectl describe pod <pod-name> -n vibe-kanban-team
```

### View Logs

```bash
# Vibe Kanban server
kubectl logs -n vibe-kanban-team -l app.kubernetes.io/name=vibe-kanban-team -f

# ElectricSQL
kubectl logs -n vibe-kanban-team -l app.kubernetes.io/component=electric -f
```

### ElectricSQL Health

```bash
kubectl port-forward -n vibe-kanban-team svc/<release>-electric 3000:3000
curl http://localhost:3000/v1/health
```

## License

This deployment configuration is provided under the MIT License.
Vibe Kanban is licensed under the [BSL License](https://github.com/BloopAI/vibe-kanban/blob/main/LICENSE).
