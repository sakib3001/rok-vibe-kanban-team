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
| `scripts/publish-npm.sh` | Build/publish the `npx` client wrapper |
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

## Add a downstream patch

```bash
cd vibe-kanban           # (with the stack applied)
# edit, then:
git format-patch -1 -o ../patches
# rename to the next NNNN-*.patch slot and append it to patches/series
```

## License

Upstream Vibe Kanban is Apache-2.0. Downstream patches/config © Rokomari.
