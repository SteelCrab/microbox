# v0.1 release checklist

## Automated

- [x] `cargo fmt --check`
- [x] unit tests
- [x] Clippy with warnings denied
- [x] Xvfb frame capture integration test
- [x] XTEST key/button/pointer integration test
- [x] application and Xvfb crash tests
- [x] broken-pipe renderer test
- [x] frame allocation limit tests
- [x] fuzz target for frame and terminal event conversion
- [x] CI workflow

## Manual before tagging

- [ ] Kitty: `microbox demo`
- [ ] Kitty: interactive `xeyes`
- [ ] Ghostty: interactive `xeyes`
- [ ] WezTerm: interactive `xeyes`
- [ ] GTK application button and text entry
- [ ] Leafpad or Mousepad editing smoke test
- [x] terminal resize at narrow, wide, and tall aspect ratios (PTY pixel-size
  capture verified on native, OCI, and Firecrab backends)
- [ ] SSH session disconnect
- [ ] demo recording or screenshot
- [ ] clean-machine installation test

## Release

- [ ] confirm README limitations and native security boundary
- [ ] confirm `docs/performance.md` on release build
- [ ] update version and changelog
- [ ] create signed `v0.1.0` tag
- [ ] publish checksums and supported terminal list
