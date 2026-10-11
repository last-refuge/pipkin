# Pinned model metadata

These JSON files are the generated provider/model catalog paired with the reference engine pin
`d2a311097cbcf669e699479587332ae3988a49d0`, extracted unchanged from
`packaging/pi-model-data-d2a311097cbcf669e699479587332ae3988a49d0.tar.zst`
(sha256 `2be5f68d90695e71372c01ccbd08d5728b93691e07c45918846997c8949bcabb`).
`manifest.json` is that archive's `.manifest.json` (sha256
`b92d631bb8bb2cac6f824a03a814baad9452164c518426b8bc4bbc5c2142bb05`); a unit test checks every file
against the manifest's per-file hashes, so an edited file fails the build's tests.

They are compiled into the engine with `include_str!`: a release build needs no live catalog
fetch and no Node tooling. Pricing is metadata, not a billing guarantee. To update, replace the
files and the manifest together from a new reviewed snapshot and record the new pair in
`llm-docs/pipkin-rust-engine-progress.md`.
