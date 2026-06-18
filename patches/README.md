# Patch Directory Structure

This directory contains the linear downstream patch stack for the shared `vibe-kanban/` checkout.

## Organization

```text
patches/
├── series
├── 0001-*.patch
├── 0002-*.patch
└── ...
```

`series` is the single source of truth. Patches apply top to bottom.

## Usage

Patches are applied automatically by CI/CD pipelines and the `scripts/apply-patches.sh` script.

### Apply patches:

```bash
./scripts/apply-patches.sh
```

### Creating a new patch:

#### Creating a new patch:
```bash
cd vibe-kanban/
# Make your changes
git add -A
git commit -m "fix: your change description"
git format-patch -1 -o ../patches/
mv ../patches/0001-your-patch.patch ../patches/NNNN-your-patch.patch
echo "NNNN-your-patch.patch" >> ../patches/series
```

## Migration Notes

**Current structure**:
- Linear patch files at `patches/*.patch`
- One ordering file: `patches/series`
- A single upstream checkout: `vibe-kanban/`

## See Also

- [ARCHITECTURE.md](../ARCHITECTURE.md) - Shared submodule architecture overview
- [scripts/apply-patches.sh](../scripts/apply-patches.sh) - Patch application script
