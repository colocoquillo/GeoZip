#!/usr/bin/env python3
import gzip
import sys
import unicodedata
from pathlib import Path

ALTERNATE_NAMES_MIN_POPULATION = 50_000
OUTPUT = Path(__file__).resolve().parent.parent / "assets" / "places.tsv.gz"


def normalize(text):
    decomposed = unicodedata.normalize("NFD", text.lower())
    return "".join(c for c in decomposed if not unicodedata.combining(c))


def is_latin(text):
    return all(c in " '-." or "LATIN" in unicodedata.name(c, "") for c in text)


def main(source):
    places = []
    with open(source, encoding="utf-8") as rows:
        for row in rows:
            fields = row.rstrip("\n").split("\t")
            name, alternates = fields[1], fields[3]
            latitude, longitude = float(fields[4]), float(fields[5])
            country, population = fields[8], int(fields[14] or 0)
            if not is_latin(name):
                name = fields[2]
            if not name:
                continue
            extra = []
            if population >= ALTERNATE_NAMES_MIN_POPULATION:
                seen = {normalize(name)}
                for alternate in alternates.split(","):
                    key = normalize(alternate)
                    if len(alternate) >= 3 and is_latin(alternate) and key not in seen:
                        seen.add(key)
                        extra.append(alternate)
            places.append((population, name, country, latitude, longitude, extra))

    places.sort(key=lambda p: (-p[0], p[1], p[2]))
    OUTPUT.parent.mkdir(exist_ok=True)
    with gzip.GzipFile(OUTPUT, "wb", compresslevel=9, mtime=0) as out:
        for population, name, country, latitude, longitude, extra in places:
            line = f"{name}\t{country}\t{latitude:.4f}\t{longitude:.4f}\t{population}\t{'|'.join(extra)}\n"
            out.write(line.encode("utf-8"))
    print(f"{len(places)} places, {OUTPUT.stat().st_size} bytes")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit("usage: build_places.py cities500.txt")
    main(sys.argv[1])
