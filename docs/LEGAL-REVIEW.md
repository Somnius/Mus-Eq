# Questions for a lawyer

Mus-Eq is a self-hosted music server (Rust server, web client, Android app) published under MIT OR Apache-2.0 (decided 2026-11-07, see `DECISIONS.md`). Nobody who works on it is a lawyer. This page lists what we could not settle by reading, with the facts we did check, so that a lawyer's time goes to the open points. Nothing here is legal advice.

## 1. Third-party programs that Mus-Eq downloads but does not ship

Mus-Eq does not include ffmpeg or yt-dlp. On the owner's request, after a typed confirmation, the server downloads a pinned build from GitHub (the BtbN `lgpl` ffmpeg build, and the official yt-dlp single-file build) and runs it as a separate process. See "Programs Mus-Eq downloads" in the README.

- **Q1.** Does Mus-Eq, as a project, have any licence or notice duty for a program that the owner's own server downloads from the original publisher at the owner's request, and that Mus-Eq never redistributes?
- **Q2.** If someone else later builds and publishes a container image that includes ffmpeg, what do they owe (licence texts, a source offer for LGPL/GPL parts)? We ship none, so this is a note for packagers.
- Checked: the ffmpeg build is configured with `--enable-version3` and without `--enable-gpl`, so we describe it as LGPL-3.0-or-later. We read this from the build's configure flags and only partly checked it against ffmpeg's own legal page. Not checked: the licences of every library inside the static build, and patent questions for AAC and other codecs.
- Not checked: yt-dlp's single-file build bundles Python and other components under their own licences.

## 2. MPL-2.0 decoders linked into the server

Sixteen `symphonia` crates (MPL-2.0) are compiled into the server binary, unmodified.

- **Q3.** For a released binary (an AUR package, an image, a release archive), is a pointer to the published crate source at the exact versions in `Cargo.lock`, plus keeping the notices, enough to meet MPL-2.0's source condition for executable form?
- Checked: none of the 16 crates' `.rs` source files carries the "Incompatible With Secondary Licenses" notice (symphonia 0.6.1). Each crate ships a `LICENSE` file.

## 3. Origin of the fingerprint code

`rusty-chromaprint` (MIT as declared) is used for duplicate detection. Its README says it is a port of Chromaprint. Upstream Chromaprint says its own source is MIT but that it bundles some FFmpeg code under LGPL 2.1, so as a whole it is LGPL 2.1.

- **Q4.** Does the port inherit any LGPL obligation, or is its MIT declaration enough?
- Checked: the port depends on the `rustfft` and `rubato` crates and its source does not mention FFmpeg, which suggests the FFmpeg-derived parts were replaced. That is evidence, not a conclusion.

## 4. Fetching tracks from streaming services

An off-by-default feature fetches one track from a YouTube Music or Spotify link through yt-dlp. It stays behind a typed acknowledgement that the owner has read the services' terms.

- **Q5.** Is shipping this feature (off by default, never run by the project, only against fakes so far) a risk for the project or its maintainer in the EU/Greece, or in other jurisdictions, for example under anti-circumvention rules or the services' terms? What wording should the README carry?

## 5. The licence choice itself

- **Q6.** MIT OR Apache-2.0 as a dual licence: anything to watch for when contributions arrive under a DCO sign-off (no CLA), in particular about relicensing later (for example to AGPL)?
- **Q7.** Is "Copyright 2026 Lef" in `LICENSE-MIT` and `NOTICE` enough, or should it name the legal person?

## 6. Third-party material in the repository

- Radio station list from Tray-FM (Unlicense), credited in `NOTICE`. Classic Winamp skins (`.wsz`) are supplied by the user and not shipped. Webamp is MIT, but the licence of its bundled base skin was not checked.
- **Q8.** The README calls the first skin "the classic Winamp look", and the player loads `.wsz` skins. Is referring to Winamp that way, and supporting its skin format, a trademark or other concern? Anything to do before release about Webamp's default skin?
