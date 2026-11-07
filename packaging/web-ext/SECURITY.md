# web-ext dependency guard

As of 2026-11-07, `web-ext` 10.8.0 depends on `deepmerge` 4.3.1 through
`addons-linter`. The npm advisory [GHSA-gcqx-w48m-w2h4](https://github.com/advisories/GHSA-gcqx-w48m-w2h4)
reports prototype poisoning and lists no patched version. This is build and
signing tooling; it is not bundled in the RDM executable or browser extension.

The CI runs the linter and signing tool with `NODE_OPTIONS=--disable-proto=throw`,
the runtime workaround described in the [upstream report](https://github.com/TehShrike/deepmerge/issues/273).
Keep this flag when running web-ext locally. The regression check is:

```sh
node --disable-proto=throw --test packaging/web-ext/security.test.cjs
```

The flag mitigates the demonstrated prototype-access vector; it does not remove
the dependency advisory. `npm audit` still reports this low-severity advisory
through three dependency nodes. CI audits at the moderate threshold while this
low-severity issue remains tracked here. Replace the mitigation with a compatible
upstream patch when one becomes available; do not downgrade web-ext to an old
major release merely to silence npm's suggested fix.
