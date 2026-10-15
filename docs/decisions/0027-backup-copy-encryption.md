# ADR 0027: Backup copies are not encrypted by Consolebook

- **Status:** Accepted
- **Date:** 2026-10-14
- **Amends:** [ADR 0024](0024-off-server-backup-copies.md) (copies at rest)

## Context

Off-server copies (ADR 0024) are plain SQLite files, like the local
snapshots (ADR 0006). A copy holds every record of the installation,
including evaluation narratives and personal data. ADR 0024 left their
protection to the agency until an encryption decision existed.

The copy exists for one situation: the server is gone. Whatever decrypts it
therefore cannot live only on the server. If Consolebook encrypted copies,
an agency would have to:

- keep a key or passphrase somewhere other than the server, for years, as
  long as the records are retained;
- hand it over, along with the share, when the people who set it up leave;
- still have it, and know where it is, on the day the server is lost.

Losing the key makes every copy unrecoverable. For a system whose purpose is
defensible records, that is worse than the exposure it prevents. Exposure
can also be prevented below Consolebook. Agencies that keep
criminal-justice-adjacent data generally already run encrypted volumes and
access-controlled shares (an encrypted NAS volume, BitLocker on the file
server, LUKS on a second disk; SMB 3 or Kerberos NFS encryption for the
network path), with key custody they already manage.

## Decision

- **Consolebook does not encrypt backup copies.** Copies stay
  plain SQLite files, so `restore` reads them with nothing but the file, and
  `doctor` and `/api/backups/status` can check them.
- **The share must provide confidentiality.** The install guide states the
  requirements:
  - storage encrypted at rest (a storage-layer control: the volume or
    dataset, not the network protocol, which protects only the path);
  - write access limited to the `consolebook` service account;
  - read access limited to the people who may restore.
  It also says that a copy holds every record.
- **Revisit** when any of these holds:
  - copies go to storage the agency does not control, such as an object
    store or a third-party host;
  - a pilot agency's policy requires encryption by the application;
  - Consolebook gains a key-custody story it already needs for another
    reason.
  An adopted design must cover key custody away from the server, recovery
  when the key holder is gone, and a tested `restore` from an encrypted copy.

## Consequences

- Recovery needs only the copy file and a Consolebook release. Nothing
  else can be lost that would make a copy unreadable.
- Confidentiality of copies depends on the agency's storage. The guide
  says so plainly rather than implying the product handles it.
- The live database and local snapshots are unchanged; ADR 0006's limits
  still apply to them.
