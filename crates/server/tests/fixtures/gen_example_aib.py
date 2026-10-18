#!/usr/bin/env python3
"""Regenerate example.aib, the Aidoku backup fixture for tests/aidoku_import.rs.

.aib is a binary plist of Aidoku's Codable Backup struct
(Aidoku/Aidoku Core/Backup/Models/Backup.swift). Swift Date maps to a plist
date (CFAbsoluteTime, seconds from 2001-01-01 UTC); python plistlib writes
naive datetimes as UTC. Field references:
- library:  BackupLibraryManga  (mangaId/sourceId are the source-ecosystem keys)
- history:  BackupHistory       (progress = last page, -1 when completed)
- manga/chapters: metadata dictionaries keyed by (id, sourceId[, mangaId])
- categories: plain strings or {title, sort, group} objects — both are valid

Run: python3 gen_example_aib.py   (writes example.aib next to this script)
"""

import datetime
import pathlib
import plistlib


def d(text):
    # naive datetime == UTC in plistlib's binary writer
    return datetime.datetime.strptime(text, "%Y-%m-%d %H:%M:%S")


BACKUP = {
    "version": "0.7",
    "name": "fixture",
    "date": d("2024-06-01 00:00:00"),
    "library": [
        # matches the manga tests register from en.example-source (key "1")
        {
            "mangaId": "1",
            "sourceId": "en.example-source",
            "lastOpened": d("2024-01-10 12:00:00"),
            "lastUpdated": d("2024-01-10 12:00:00"),
            "dateAdded": d("2024-01-10 12:00:00"),  # unix 1704888000
            "lastRead": d("2024-02-01 12:00:00"),
            "categories": ["Evening", "Reading"],
        },
        # not registered by tests; created by the import with metadata below
        {
            "mangaId": "2",
            "sourceId": "en.example-source",
            "lastOpened": d("2024-01-11 08:00:00"),
            "lastUpdated": d("2024-01-11 08:00:00"),
            "dateAdded": d("2024-01-11 08:00:00"),  # unix 1704960000
            "categories": ["Reading", "Favorites"],  # Favorites: membership-only category
        },
        # source not installed -> skipped
        {
            "mangaId": "ghost-1",
            "sourceId": "en.ghost-source",
            "lastOpened": d("2024-01-12 08:00:00"),
            "lastUpdated": d("2024-01-12 08:00:00"),
            "dateAdded": d("2024-01-12 08:00:00"),
            "categories": ["Evening"],
        },
    ],
    "manga": [
        {
            "id": "1",
            "sourceId": "en.example-source",
            "title": "Aib Title One",
            "author": "Author A",
            "artist": "Artist A",
            "desc": "desc one",
            "tags": ["Action", "Fantasy"],
            "cover": "https://example.com/c1.jpg",
            "url": "https://example.com/manga/1",
            "status": 1,
            "nsfw": 0,
            "viewer": 0,
        },
        {
            "id": "2",
            "sourceId": "en.example-source",
            "title": "Second Manga",
            "author": "Author B",
            "status": 2,
            "nsfw": 2,
            "viewer": 0,
        },
    ],
    "chapters": [
        {
            "id": "3",
            "sourceId": "en.example-source",
            "mangaId": "1",
            "title": "Ch 3",
            "chapter": 3.0,
            "lang": "en",
            "sourceOrder": 6,
            "dateUploaded": d("2024-01-03 00:00:00"),
        },
        {
            "id": "7",
            "sourceId": "en.example-source",
            "mangaId": "1",
            "title": "Ch 7",
            "chapter": 7.0,
            "lang": "en",
            "sourceOrder": 2,
            "dateUploaded": d("2024-01-07 00:00:00"),
        },
        {
            "id": "1",
            "sourceId": "en.example-source",
            "mangaId": "2",
            "title": "Ch 1",
            "chapter": 1.0,
            "lang": "en",
            "sourceOrder": 1,
        },
        {
            "id": "9",
            "sourceId": "en.ghost-source",
            "mangaId": "ghost-1",
            "lang": "en",
            "sourceOrder": 1,
        },
    ],
    "history": [
        # tests seed lastPageRead=9 for this chapter: max() must keep 9
        {
            "sourceId": "en.example-source",
            "mangaId": "1",
            "chapterId": "3",
            "progress": 5,
            "completed": False,
            "dateRead": d("2024-02-01 10:00:00"),  # unix 1706781600
        },
        # -1 + completed is Aidoku's "finished" marker
        {
            "sourceId": "en.example-source",
            "mangaId": "1",
            "chapterId": "7",
            "progress": -1,
            "completed": True,
            "dateRead": d("2024-02-02 10:00:00"),  # unix 1706868000
        },
        # history for a manga that is only created during this import
        {
            "sourceId": "en.example-source",
            "mangaId": "2",
            "chapterId": "1",
            "progress": 2,
            "completed": False,
            "dateRead": d("2024-02-03 10:00:00"),  # unix 1706954400
        },
        # source not installed -> skipped
        {
            "sourceId": "en.ghost-source",
            "mangaId": "ghost-1",
            "chapterId": "9",
            "progress": 1,
            "completed": False,
            "dateRead": d("2024-02-04 10:00:00"),
        },
    ],
    # both plist shapes Aidoku emits: bare string and object
    "categories": ["Evening", {"title": "Reading", "sort": 3, "group": False}],
}


def main():
    out = pathlib.Path(__file__).with_name("example.aib")
    with out.open("wb") as fh:
        plistlib.dump(BACKUP, fh, fmt=plistlib.FMT_BINARY, sort_keys=False)
    print(f"wrote {out} ({out.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
