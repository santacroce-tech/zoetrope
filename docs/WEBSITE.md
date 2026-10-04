# Website and releases

## The website (GitHub Pages)

<https://santacroce-tech.github.io/zoetrope/> is built by
`scripts/build-site.sh` into `site/dist` and deployed by
`.github/workflows/pages.yml` on every push to `main`.

| Path | Content | Source |
|------|---------|--------|
| `/` | Landing page | `site/src/index.html` |
| `/manual/` | Manual and tutorials | `site/src/manual/*.html` |
| `/demos/` | Live exported demos, embedded on the landing page | `editor/tools/export-demos.ts`, which uses the same export code as the app |
| `/app/` | The full web editor | `editor/`, built with a relative base |

- **Build:** `site/build.mjs` wraps each page, an HTML fragment with a
  `title`/`description` comment, in `site/src/_layout.html`. It adds the
  manual sidebar and the previous/next links, and has no dependencies.
- **Adding a manual page:** add it to `MANUAL` in `build.mjs`.
- **Screenshots:** they live in `site/src/assets/img` as WebP. They were
  captured from the editor at 1440×900.
- **Download buttons:** `site/src/assets/site.js` asks the GitHub API for the
  latest release, links each platform's files, and highlights the visitor's
  OS. If the API fails, the buttons point at the releases page.

Preview locally:

```sh
scripts/build-site.sh
python3 -m http.server -d site/dist 8080   # http://localhost:8080
```

## Releases

`.github/workflows/release.yml` runs when a version tag is pushed. It builds
on GitHub's runners and publishes the files to a GitHub Release:

| Platform | Runner | Files |
|----------|--------|-------|
| macOS (Apple Silicon and Intel) | `macos-latest`, `--target universal-apple-darwin` | `.dmg`, `.app.tar.gz` |
| Windows (x64) | `windows-latest` | `-setup.exe` (NSIS), `.msi` |
| Linux (x64) | `ubuntu-22.04` (WebKitGTK 4.1) | `.AppImage`, `.deb`, `.rpm` |

To release:

1. Bump the version in `editor/src-tauri/tauri.conf.json`,
   `editor/package.json` and `Cargo.toml` (`[workspace.package]`).
2. Merge to `main` with CI green.
3. Run `git tag vX.Y.Z && git push origin vX.Y.Z`.

The website picks up the new release automatically.

**Code signing (not set up yet).** The builds are unsigned, so macOS
Gatekeeper and Windows SmartScreen warn on first launch; the manual explains
how to get past this. Signing needs an Apple Developer ID certificate plus
notarization credentials, and a Windows code-signing certificate, stored as
repository secrets. `tauri-action` picks them up from its documented
environment variables.
