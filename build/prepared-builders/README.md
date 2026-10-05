# Prepared stable Linux environment

All four Linux compilation jobs in CI, check-full and release select a prepared
nonroot fleet runner, with a hosted Ubuntu 24.04 job-container fallback. Mac jobs
and newest-Ubuntu archive-only tests retain their original environments.

The generic toolchain is named `rust-stable-linux` in homelab-infra's
`runner/builder/catalog.json`. It contains stable Rust, rustfmt and Clippy; there
is no Swamp source, crate dependency cache or binary in the image. Current baked
stable is 1.99.0. Immutable tools and runner digests live in the catalog and
workflows. Publication under Swamp's GHCR package gives this repository's job
token package access; another repository must receive its own read grant and
broker approval. This is an existing package, not a separate code repository.

Activation refreshes stable and retains the original floating policy. When stable
is unchanged, installed tools are reused. A future stable release may download
new components per ephemeral job until another image is prepared/promoted.
`RUSTUP_TOOLCHAIN=stable` selects the job's actual prepared toolchain without
installing the unrelated Mac target from the workspace-wide toolchain file.

Fleet jobs use the existing NAS sccache service. The namespace includes repository,
runner identity and active compiler commit; mutable Cargo target state stays
isolated per job. Hosted jobs retain the original Swatinem cache. No other
repository's target snapshot or credentials are baked/copied. The broker approval
allocates 8 CPUs/12 GiB; both fleet hosts pull one NAS-published runner image.

To prepare current stable again:

```sh
gh workflow run prepare-builders.yml
```

The factory resolves Rust's official stable manifest, converts the reviewed
canonical Actions setup in `source.yml` using `profile.json`, reuses or publishes
the content-tagged GHCR image, and exports tools and recipes. Its build verifies
that stable still matches the resolved release; if stable advances mid-build,
rerun. Image preparation never runs the workspace build.

Publish the runner derivative once on an approved Docker publisher: load the
`prepared-tools` artifact, build `Dockerfile.runner` from `prepared-builder`, push
the manifest's runner tag, then pull its promoted digest on both fleet hosts.
Update the catalog/workflow pins and broker approval together after review and
representative CI. Preserve existing approvals; generated `broker-policy.json`
adds this consumer only and must not replace the whole policy.

`requirements.json`, `access.json`, `cache.json` and `broker-policy.json` are the
reviewed adoption records. `builder reuse rust-stable-linux` in homelab-infra
handles another compatible Linux Actions job without rebuilding this image.
