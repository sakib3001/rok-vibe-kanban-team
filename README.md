# Vibe Kanban — Rokomari (code + downstream patches)

Upstream [Vibe Kanban](https://github.com/BloopAI/vibe-kanban) source plus Rokomari's
downstream patch stack. Deployed via **Docker Compose** (no Helm/Kubernetes).

## Layout

| Path | What |
|------|------|
| `vibe-kanban/` | Upstream source (git submodule, pinned tag) |
| `patches/` | Downstream patch stack — `series` (order) + `NNNN-*.patch`, applied at build time |
| `scripts/apply-patches.sh` | Apply the patch stack to `vibe-kanban/` |
| `scripts/update-vibe-kanban.sh` | Bump the upstream submodule |
| `scripts/publish-npm.sh` | Build/publish the `npx` client wrapper + upload binaries to R2 |
| `scripts/build-portable-linux.sh` | Rebuild only the linux `vibe-kanban` binary (glibc 2.31) and stage it for R2 |
| `scripts/Dockerfile.portable-linux` | Builds the linux binaries on Debian bullseye (glibc 2.31) for broad Ubuntu support |
| `mail-templates/loops/` | Loops transactional email templates (invite / review) |

Notable downstream patches: Zoho OAuth (provider + sign-in buttons), GitLab MRs,
allowed-email-domains, browser notifications, executor pins, and the remote-web
fixes (`0040` Zoho buttons, `0041` invite-complete redirect).

## Deployment & ops

The Docker Compose stack and operational docs live at the repository root:

- `./docker-compose.yml` + `./DEPLOYMENT_README.md` — Compose stack docs and configuration
- `./scripts/invite.sh` — create org invitations + print accept links
- `./sql/protect-service-account.sql` — DB guard for the service account
- `./SYSTEM_DESIGN.md`, `./EXECUTION_PLAN.md`, `./DEVELOPER_ONBOARDING.md`

## Build a patched remote image

```bash
git submodule update --init vibe-kanban
scripts/apply-patches.sh vibe-kanban
docker build -f vibe-kanban/crates/remote/Dockerfile -t <registry>/vibe-kanban-remote:<tag> vibe-kanban
```

## Build & publish the client binaries (npm + R2)

The `npx` client (`vibe-kanban-team`) downloads a prebuilt `vibe-kanban` (server)
binary from R2 (`https://rokfiles.rokomari.io`) on first run and runs it locally.

**Linux portability rule:** these binaries are built inside **Debian bullseye
(glibc 2.31)** via `scripts/Dockerfile.portable-linux`, *not* natively. glibc is
backward- but not forward-compatible, so a binary linked against a newer glibc
(e.g. 2.39 on an Ubuntu 24.04 build host) fails to even load on Ubuntu 22.04 /
20.04 with `Command failed` / `GLIBC_2.xx not found`. Building on bullseye yields
a max requirement of `GLIBC_2.30`, which runs on Ubuntu **20.04 / 22.04 / 24.04**.
`scripts/publish-npm.sh` does this automatically for `x86_64-linux` (it shells out
to Docker for the `server` / `vibe-kanban-mcp` / `review` bins); macOS stays native.
Requires Docker on the build host.

### Full release (build all three bins + upload + npm publish)

```bash
# Needs R2_* + NPM_TOKEN + VITE_PUBLIC_REACT_VIRTUOSO_LICENSE_KEY
# (see scripts/publish-credentials.bashrc, gitignored).
scripts/publish-npm.sh
```

### Rebuild only the linux server binary for an existing release

Use this to fix glibc compatibility for an already-published tag without
re-publishing to npm. It builds, asserts max glibc ≤ 2.31, and refreshes the R2
staging dir under `rok-vibe-kanban-launcher/r2-upload-rokfiles/`:

```bash
scripts/build-portable-linux.sh
```

Then upload the staged zip + manifest to R2 and clear any client's cache so it
re-downloads (the script prints the exact commands):

```bash
cd rok-vibe-kanban-launcher/r2-upload-rokfiles
aws --endpoint-url "$R2_ENDPOINT" s3 cp binaries/<TAG>/linux-x64/vibe-kanban.zip \
  "s3://$R2_BUCKET/binaries/<TAG>/linux-x64/vibe-kanban.zip"
aws --endpoint-url "$R2_ENDPOINT" s3 cp binaries/<TAG>/manifest.json \
  "s3://$R2_BUCKET/binaries/<TAG>/manifest.json" --content-type application/json

# On a machine that cached the bad binary:
rm -rf ~/.vibe-kanban/bin/<TAG> && systemctl --user restart vibe-kanban
```

## Add a downstream patch

```bash
cd vibe-kanban           # (with the stack applied)
# edit, then:
git format-patch -1 -o ../patches
# rename to the next NNNN-*.patch slot and append it to patches/series
```

## License

Upstream Vibe Kanban is Apache-2.0. Downstream patches/config © Rokomari.
