# Next Steps

## Immediate (before first push)

### 1. SHA-pin all GitHub Actions `uses:` entries (REQUIRED — Criterion 1 gate)

Every third-party action must be pinned to a full 40-character SHA with a `# vX.Y.Z`
comment. Current tag-only pins that must be fixed (do NOT auto-fix — look up the
correct SHA for each):

**`.github/workflows/ci.yml`**:
- Line 31: `actions/checkout@v4`
- Line 34: `dtolnay/rust-toolchain@stable`
- Line 40: `actions/cache@v4`
- Line 63: `actions/checkout@v4`
- Line 66: `dtolnay/rust-toolchain@stable`
- Line 71: `actions/cache@v4`
- Line 118: `actions/checkout@v4`
- Line 157: `dtolnay/rust-toolchain@stable`
- Line 162: `actions/cache@v4`
- Line 204: `actions/checkout@v4`
- Line 235: `dtolnay/rust-toolchain@stable`
- Line 240: `actions/cache@v4`
- Line 256: `codecov/codecov-action@v4`
- Line 278: `actions/checkout@v4`
- Line 281: `docker/setup-buildx-action@v3`

**`.github/workflows/security.yml`**:
- Line 18: `actions/checkout@v4`
- Line 21: `dtolnay/rust-toolchain@stable`
- Line 35: `actions/checkout@v4`

### 2. Add Dependabot cooldown (REQUIRED)

`.github/dependabot.yml` must include `cooldown.default-days: 21` (or higher) under
each `updates` entry. Current config has only `interval: "weekly"` — no cooldown.

### 3. Push and open PR

```bash
git push -u origin main
# then open a PR on GitHub
```

### 4. Verify CI passes on remote

After push: confirm all five CI jobs (lint, build, integration, coverage, docker)
go green. The integration job requires the Postgres service container — it will not
run locally.

### 5. Fix README.md prettier formatting

`npx prettier --write README.md` — informational now, clean it before the repo
gets broader contributors so it doesn't accumulate.

## Ongoing

- Keep `sz-rust-sdk` rev-pin current when the SDK cuts new releases.
- Revisit `mssql` and `both` Docker matrix entries once Microsoft re-signs their
  apt repo key (see TODO comment in `ci.yml` line ~274).
- Add `MSSQL` integration test variant when the Docker matrix is restored.
