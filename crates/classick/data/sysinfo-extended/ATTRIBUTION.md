# SysInfoExtended templates

Source: https://github.com/dstaley/ipod-sysinfo (License: CC0-1.0, public domain).
These are per-model iPod device-capability plists. classick writes the matching
one to `iPod_Control/Device/SysInfoExtended` so libgpod generates the correct
artwork thumbnail (`.ithmb`) formats. GUID is injected at write time per device.

| File | Source path |
|---|---|
| `classic-late2009.plist` | `models/iPod/6th generation/Late_2009_SysInfoExtended.plist` |
| `classic-6g.plist` | `models/iPod/6th generation/SysInfoExtended.plist` |
| `video-5g.plist` | `models/iPod/5th generation/SysInfoExtended.plist` |
| `photo-4g.plist` | `models/iPod/4th generation/Photo_SysInfoExtended.plist` (the plain `SysInfoExtended.plist` in that folder lacks `ImageSpecifications`, so the Photo-specific file was used instead) |
| `nano-1g.plist` | `models/iPod nano/1st generation/SysInfoExtended.plist` |
| `nano-2g.plist` | `models/iPod nano/2nd generation/SysInfoExtended.plist` |
| `nano-3g.plist` | `models/iPod nano/3rd generation/SysInfoExtended.plist` |
| `nano-4g.plist` | `models/iPod nano/4th generation/SysInfoExtended.plist` |
