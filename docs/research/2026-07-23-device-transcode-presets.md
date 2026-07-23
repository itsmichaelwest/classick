# Device transcode presets for Classick V1

Research date: 2026-07-23. This records the evidence and product rationale for
the V1 user-facing transcode profiles.

## Decision

Expose four named output profiles, all producing an `.m4a` file:

| Profile | Encoder/output | Intended use |
| --- | --- | --- |
| Lossless | ALAC (current behaviour) | Preserve source fidelity; storage is plentiful. |
| High quality **(recommended default for lossy)** | AAC-LC, 256 kbps | A conservative portable-listening choice. |
| Balanced | AAC-LC, 192 kbps | Substantially more capacity with a cautious quality margin. |
| More music | AAC-LC, 128 kbps | Small-capacity iPods and casual/noisy listening. |

Do **not** expose MP3 output or a 320-kbps AAC preset in V1. The first
generation nano can play both formats, but AAC is Apple’s own device-sync
conversion target and Apple describes AAC as suitable for most music; MP3 is
positioned for playback outside the Apple ecosystem. A 320-kbps preset adds
storage cost without a comparably clear portable-use case. Keep a future
advanced custom-rate control out of V1 unless a concrete user need emerges.

The app should say that these are *device copies*: switching profiles requires
a re-sync and does not alter source files. That mirrors Apple’s present sync
behaviour. Keep **Lossless** as the V1 default so the new setting preserves
Classick's intended high-fidelity behaviour unless the user deliberately
chooses a space-saving profile. There is no migration requirement because V1
has not shipped.

Expose the same four profiles for every supported device. Do not infer,
restrict, hide, disable, or change the default profile from model, generation,
capacity, or capability data. Hardware is relevant to validation and storage
estimates only; the user owns the per-device output choice.

## Apple device and sync evidence

### First-generation iPod nano is within range

Apple’s archived first-generation nano specification lists AAC and MP3,
including VBR MP3, at **16–320 kbps**, plus Apple Lossless, AIFF and WAV. It
lists 1, 2 and 4 GB models, and explicitly bases its advertised 240–1,000 song
capacity on four-minute **128-kbps AAC** tracks. This establishes that every
proposed AAC rate is supported and that 128 kbps is historically an Apple
capacity baseline—not a claim of universal transparency.

- [iPod nano (first generation) technical specifications — Apple](https://support.apple.com/en-euro/112507)
- [2005 launch announcement — Apple](https://www.apple.com/newsroom/2005/09/07Apple-Introduces-iPod-nano/)

Apple’s original spec does not name an AAC profile. Encode **AAC-LC** rather
than HE-AAC: the first-generation page names only “AAC,” whereas Apple’s later
seventh-generation nano specification separately names HE-AAC. This is a
compatibility-conservative inference, so physical-device testing remains the
release gate.

### The conversion model remains current

Current Apple documentation for both macOS Finder sync and Windows Apple
Devices says that “Convert higher bit rate songs to” makes a smaller device
copy while leaving the library original unchanged. It intentionally does not
publish the menu’s available rates. Current iTunes for Windows documentation
still documents **128-kbps AAC** as the space-maximising option for iPod
shuffle. Historical Apple Support Community reports describe the familiar
128/192/256-kbps device-sync picker, but those reports are user/community
evidence rather than a normative Apple specification.

- [Save storage space when syncing on Mac — Apple](https://support.apple.com/en-mide/guide/mac-help/mchl0c0decb9/mac)
- [Problems syncing music or video on Windows — Apple](https://support.apple.com/en-gb/guide/devices-windows/mchld64e6159/windows)
- [Manage iPod shuffle in iTunes on PC — Apple](https://support.apple.com/en-qa/guide/itunes/itns3206/windows)
- [Choose import settings in iTunes on PC — Apple](https://support.apple.com/guide/itunes/choose-import-settings-itns2965/windows)
- [Historical 128/192/256 device-sync picker discussion — Apple Support Community](https://discussions.apple.com/thread/5625862)

## Listening-quality evidence and limits

The sources below are useful expert-community evidence, not product
requirements. Codec generation, music, playback chain, hearing, and test
method all matter; bitrate alone does not prove audibility.

* **128 kbps AAC-LC:** Apple used it for the original nano’s capacity figures
  and its modern iTunes shuffle guidance. A 2005 public, blind multiformat
  listening test from the Hydrogenaudio community found contemporary iTunes
  AAC and LAME MP3 broadly tied at roughly this rate (the iTunes files actually
  averaged 137.56 kbps), and called the quality very good. That is stronger
  than an anecdote, but it is not a current-encoder test, a device test, or a
  universal-transparency result. Later forum discussion also warns that
  difficult samples and trained listeners can expose artifacts. Classick
  should label it capacity-first, not “transparent.”
* **192 kbps AAC-LC:** A reasonable cautious middle ground. Hydrogenaudio
  discussion reports it as transparent on most samples, with exceptions for
  problem material; that is informed listening commentary, not a controlled
  universal threshold. It earns a preset because it is a meaningful capacity
  step from 256, not because it guarantees transparency.
* **256 kbps AAC-LC:** Best default for a lossy profile. It matches the
  long-standing upper end of Apple’s historical sync picker reported by Apple
  Community users, gives margin over 192 for hard material, and remains far
  smaller than ALAC. This is a conservative product decision, not evidence
  that 256 is audibly superior for every listener.
* **MP3:** The controlled 128-kbps test does not establish an across-the-board
  AAC win: its iTunes AAC and LAME MP3 results were broadly tied. Still, since
  this iPod supports AAC natively, Classick controls the output, and Apple
  makes AAC its general-purpose import/device choice, MP3 would create an
  extra V1 choice without a compatibility benefit.

Representative sources, with evidence type called out:

- [Hydrogenaudio 128-kbps AAC public listening-test discussion](https://hydrogenaudio.org/index.php/topic%2C10457.0.html) — public-test discussion; useful historical context, not a current codec benchmark.
- [Hydrogenaudio public multiformat listening test at about 128 kbps](https://listening-tests.hydrogenaudio.org/sebastian/mf-128-1/results.htm) — blind multi-listener results; the most useful controlled evidence here, with 2005 encoder and selected-sample limits.
- [Hydrogenaudio 96/128-kbps AAC test discussion](https://hydrogenaudio.org/index.php/topic%2C77809.0.html) — test planning/results discussion; explicitly highlights critical samples and listener variance.
- [Hydrogenaudio AAC vs. LAME MP3 discussion](https://hydrogenaudio.org/index.php/topic%2C61667.0.html) — expert community consensus/advice, not a controlled result.
- [Hydrogenaudio 224-kbps AAC discussion](https://hydrogenaudio.org/index.php/topic%2C52951.0.html) — useful corrective: claims of audibility should be ABX-tested, not inferred from confidence or equipment.

## Product and implementation notes

1. Keep source audio read-only and identify the setting as an output
   *transcode profile*, not an import-quality setting.
2. Encode AAC-LC in an MP4/M4A container; verify each profile on a
   first-generation nano as part of device testing (load, index, play full
   tracks, seek, and resume). Apple’s format/rate table establishes capability,
   not Classick’s encoder/container/tagging interoperability. This testing is
   a release-confidence exercise, not a runtime capability gate.
3. Present expected size as an estimate from track duration and target rate,
   rather than a song-count promise. For a four-minute track, nominal audio
   payload is about 3.8 MB at 128 kbps, 5.8 MB at 192 kbps, and 7.7 MB at 256
   kbps, before container, artwork, and database overhead.
4. Do not transcode an already-lossy source by default without an explicit
   user decision; successive lossy encodes can add artifacts. This is a
   separate source-policy decision from selecting the output profile.

## Protocol shape

Represent the portable, per-device choice as one closed enum:

```json
{
  "transcode_profile": "alac"
}
```

V1 values:

- `alac`
- `aac_256`
- `aac_192`
- `aac_128`

Make the field required in the V1 device-settings schema and default it to
`alac` only when constructing a new device profile. Do not encode a separate
codec plus arbitrary bitrate: the four supported combinations are the product
contract, and a closed enum prevents unsupported or nonsensical combinations.

Keep the existing `ffmpeg`/`refalac` encoder selection separate and host-local.
It chooses the implementation used to produce ALAC; it does not describe the
device output. In particular, `refalac` is not an alternative AAC encoder.
That host setting can later be retired without changing the portable
`transcode_profile` contract.

## Confidence

**High** for device format/rate support and the Apple device-copy model, based
on Apple documentation. **Medium** for the exact historical 128/192/256 picker
set and for listening recommendations: the picker values are corroborated by
Apple Community reports, and listening quality necessarily varies by material
and listener. **Low until physical validation** for first-generation-nano
interoperability of the exact encoder, container metadata, artwork and all
profile outputs.
