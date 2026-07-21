# Redacted iPod readiness layouts

These path-only fixtures were derived from the ignored 2026-07-19 physical
captures of the factory-restored and Finder-initialized iPod layouts. File
contents and private identifiers were deliberately excluded: the fixtures do
not contain a device GUID, printed serial, owner/device/host name, volume UUID,
dynamic SCSI value, or absolute host path.

Each non-comment line is `D relative/path` for a directory or
`F relative/path` for an empty file marker. The initialized fixture therefore
does not contain a usable iTunesDB. Readiness tests use an injected validator
for the structural-ready unit case because the empty path marker is not a
database; the production-validator test proves the marker is invalid.
