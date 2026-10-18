#!/usr/bin/env python3
"""Regenerate the Suwayomi/Mihon backup fixtures for tests/suwayomi_import.rs.

Wire schema (field numbers verified against both codebases):
- Suwayomi-Server: server/.../backup/proto/models/{Backup,BackupManga,
  BackupChapter,BackupCategory,BackupSource,BackupHistory}.kt
- Mihon: app/.../data/backup/models/*.kt
Container reality (both apps): the file is gzip(Backup proto) — Suwayomi's
ProtoBackupExport and Mihon's BackupCreator both pipe through okio gzip;
Mihon's BackupDecoder also accepts a raw proto (sniffs the 0x1f8b magic).
The zip variant below is a defensive container some forks are rumored to
use; the importer sniffs all three (gzip / zip / raw).

Timestamps are epoch milliseconds (0 = absent); manga.categories holds
category *order* values, not positions (Mihon MangaRestorer maps by order);
kotlinx ProtoBuf omits default-valued scalars, so favorite=true never hits
the wire — decoder default is true.

No third-party deps: the wire format is hand-rolled (varint + length
delimiters). Run: python3 gen_example_tachibk.py
"""

import gzip
import pathlib
import struct
import zipfile

W_VARINT = 0
W_FIXED32 = 5
W_LEN = 2


def varint(n):
    out = bytearray()
    while True:
        b = n & 0x7F
        n >>= 7
        if n:
            out.append(b | 0x80)
        else:
            out.append(b)
            return bytes(out)


def key(field, wire):
    return varint((field << 3) | wire)


def f_varint(field, n):
    return key(field, W_VARINT) + varint(n)


def f_bool(field, b):
    return f_varint(field, 1 if b else 0)


def f_float(field, f):
    return key(field, W_FIXED32) + struct.pack("<f", f)


def f_str(field, s):
    b = s.encode()
    return key(field, W_LEN) + varint(len(b)) + b


def f_bytes(field, b):
    return key(field, W_LEN) + varint(len(b)) + b


def f_msg(field, payload):
    return f_bytes(field, payload)


def f_packed_ints(field, ns):
    payload = b"".join(varint(n) for n in ns)
    return f_bytes(field, payload)


def chapter(url, name, number=0.0, read=False, bookmark=False, last_page=0,
            date_upload=0, source_order=0, scanlator=None, memo=None):
    out = f_str(1, url) + f_str(2, name)
    if scanlator is not None:
        out += f_str(3, scanlator)
    if read:
        out += f_bool(4, True)
    if bookmark:
        out += f_bool(5, True)
    if last_page:
        out += f_varint(6, last_page)
    if date_upload:
        out += f_varint(8, date_upload)
    if number:
        out += f_float(9, number)
    if source_order:
        out += f_varint(10, source_order)
    if memo is not None:
        out += f_bytes(13, memo)
    return out


def history(url, last_read):
    return f_str(1, url) + f_varint(2, last_read)


def manga(source, url, title, artist=None, author=None, desc=None, genre=(),
          status=0, cover=None, date_added=0, categories=(), packed=True,
          favorite=True, chapters=(), histories=()):
    out = f_varint(1, source) + f_str(2, url) + f_str(3, title)
    if artist is not None:
        out += f_str(4, artist)
    if author is not None:
        out += f_str(5, author)
    if desc is not None:
        out += f_str(6, desc)
    for g in genre:
        out += f_str(7, g)
    if status:
        out += f_varint(8, status)
    if cover is not None:
        out += f_str(9, cover)
    if date_added:
        out += f_varint(13, date_added)
    for c in chapters:
        out += f_msg(16, c)
    if packed:
        if categories:
            out += f_packed_ints(17, categories)
    else:
        for c in categories:
            out += f_varint(17, c)
    # tracking (18): declared in the schema, never consumed by the importer —
    # present to prove unknown messages are skipped
    out += f_msg(18, f_varint(1, 1) + f_str(2, "track:example"))
    if not favorite:
        out += f_bool(100, False)
    for h in histories:
        out += f_msg(104, h)
    # suwayomi meta map (9000): likewise out of scope, must be skipped
    out += f_msg(9000, f_str(1, "k") + f_str(2, "v"))
    return out


def category(name, order):
    return f_str(1, name) + f_varint(2, order)


def source(name, source_id):
    return f_str(1, name) + f_varint(2, source_id)


def ms(text):
    import datetime
    dt = datetime.datetime.strptime(text, "%Y-%m-%d %H:%M:%S")
    return int(dt.replace(tzinfo=datetime.timezone.utc).timestamp() * 1000)


# source ids are int64 hashes upstream; 111/222 keep the fixture readable
SOURCE_EXAMPLE = 111  # matches installed "Example Source" (en.example-source)
SOURCE_GHOST = 222    # no installed source with this name

MANGA1 = manga(
    SOURCE_EXAMPLE, "1", "Tachibk Title One",
    artist="Artist A", author="Author A", desc="desc one",
    genre=["Action", "Fantasy"], status=1, cover="https://example.com/c1.jpg",
    date_added=ms("2024-01-10 12:00:00"),  # unix 1704888000
    categories=[0, 1],  # Evening, Reading
    chapters=[
        # tests seed lastPageRead=9: max() must keep 9; bookmark must NOT import
        chapter("3", "Ch 3", number=3.0, last_page=5, bookmark=True,
                date_upload=ms("2024-01-03 00:00:00"), source_order=6,
                scanlator="Scan A", memo=b"{}"),
        chapter("7", "Ch 7", number=7.0, read=True,
                date_upload=ms("2024-01-07 00:00:00"), source_order=2),
    ],
    histories=[
        history("3", ms("2024-02-01 10:00:00")),  # unix 1706781600
        history("7", ms("2024-02-02 10:00:00")),  # unix 1706868000
    ],
)

MANGA2 = manga(
    SOURCE_EXAMPLE, "2", "Second Manga",
    author="Author B", status=2,
    date_added=ms("2024-01-11 08:00:00"),  # unix 1704960000
    categories=[1, 2], packed=False,  # Reading, Favorites; unpacked encoding
    chapters=[chapter("1", "Ch 1", number=1.0, last_page=2, source_order=1)],
    histories=[history("1", ms("2024-02-03 10:00:00"))],  # unix 1706954400
)

GHOST = manga(
    SOURCE_GHOST, "ghost-1", "Ghost Manga",
    categories=[0],
    chapters=[chapter("9", "Ch 9", read=True, last_page=3)],
    histories=[history("9", ms("2024-02-04 10:00:00"))],
)

# not favorite (Mihon "read entries" option): never enters the library, but
# its history still lands; chapter 5 exists only in history, not chapters
HISTORY_ONLY = manga(
    SOURCE_EXAMPLE, "3", "",
    favorite=False,
    histories=[history("5", ms("2024-02-05 10:00:00"))],  # unix 1707127200
)


def build_backup():
    out = b""
    for m in (MANGA1, MANGA2, GHOST, HISTORY_ONLY):
        out += f_msg(1, m)
    for c in (category("Evening", 0), category("Reading", 1), category("Favorites", 2)):
        out += f_msg(2, c)
    out += f_msg(101, source("Example Source", SOURCE_EXAMPLE))
    out += f_msg(101, source("Ghost Source", SOURCE_GHOST))
    # mihon backupPreferences (104) / suwayomi meta (9000): out of scope
    out += f_msg(104, f_str(1, "pref_key") + f_msg(2, f_str(1, "x")))
    out += f_msg(9000, f_str(1, "k") + f_str(2, "v"))
    return out


def main():
    here = pathlib.Path(__file__).parent
    proto = build_backup()

    tachibk = here / "example.tachibk"
    tachibk.write_bytes(gzip.compress(proto, mtime=0))

    raw = here / "example.proto"
    raw.write_bytes(proto)

    zipped = here / "example_zip.tachibk"
    with zipfile.ZipFile(zipped, "w", zipfile.ZIP_DEFLATED) as zf:
        # entry holds gzipped proto: proves the importer re-sniffs after unzip
        zf.writestr("backup.proto", gzip.compress(proto, mtime=0))

    for f in (tachibk, raw, zipped):
        print(f"wrote {f} ({f.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
