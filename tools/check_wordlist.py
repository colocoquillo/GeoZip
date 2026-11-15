#!/usr/bin/env python3
import os
import re
import sys
import unicodedata

EXPECTED_ROWS = 1850
HEADER = ["id", "en", "es", "en_alias", "es_alias"]
RAW_EN = re.compile(r"^[a-z]+$")
RAW_ES = re.compile(r"^[a-záéíóúüñ]+$")
NORM = re.compile(r"^[a-z]{3,14}$")
DEFAULT = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
                       "wordlist", "gz1-concepts.tsv")


def normalise(word):
    decomposed = unicodedata.normalize("NFD", word.lower())
    return "".join(c for c in decomposed if not unicodedata.combining(c))


def main(argv):
    path = argv[1] if len(argv) > 1 else DEFAULT
    errors = []

    def err(msg):
        errors.append(msg)

    try:
        with open(path, "rb") as fh:
            raw = fh.read()
    except OSError as exc:
        print("ERROR: cannot read %s: %s" % (path, exc))
        return 2
    if b"\r" in raw:
        err("file: contains carriage returns (must use Unix newlines)")
    if raw.startswith(b"\xef\xbb\xbf"):
        err("file: starts with a UTF-8 BOM")
    if not raw.endswith(b"\n"):
        err("file: does not end with a newline")
    try:
        text = raw.decode("utf-8")
    except UnicodeDecodeError as exc:
        print("ERROR: %s is not valid UTF-8: %s" % (path, exc))
        return 2
    if unicodedata.normalize("NFC", text) != text:
        err("file: text is not in Unicode NFC form")

    lines = text.split("\n")
    if lines and lines[-1] == "":
        lines.pop()
    if not lines or lines[0].split("\t") != HEADER:
        err("header: first line must be exactly %r" % "\t".join(HEADER))
    rows = lines[1:]
    if len(rows) != EXPECTED_ROWS:
        err("rows: expected %d data rows, found %d" % (EXPECTED_ROWS, len(rows)))

    en_seen = {}
    es_seen = {}
    prev_key = None
    prev_en = None
    n_en_alias = n_es_alias = 0

    for index, line in enumerate(rows):
        lineno = index + 2
        fields = line.split("\t")
        if len(fields) != 5:
            err("line %d: expected 5 tab-separated fields, found %d" % (lineno, len(fields)))
            continue
        rid, en, es, en_alias, es_alias = fields
        if rid != str(index):
            err("line %d: id is %r, expected %d" % (lineno, rid, index))
        tag = "id %s" % rid

        def check_word(word, lang, kind):
            raw_re = RAW_EN if lang == "en" else RAW_ES
            if word != word.strip() or not word:
                err("%s: %s %s %r is empty or has surrounding whitespace" % (tag, lang, kind, word))
                return None
            if not raw_re.match(word):
                err("%s: %s %s %r has characters outside the allowed lowercase alphabet" % (tag, lang, kind, word))
            norm = normalise(word)
            if not NORM.match(norm):
                err("%s: %s %s %r normalises to %r which does not match ^[a-z]{3,14}$" % (tag, lang, kind, word, norm))
            return norm

        en_norm = check_word(en, "en", "word")
        es_norm = check_word(es, "es", "word")
        en_aliases = en_alias.split("|") if en_alias else []
        es_aliases = es_alias.split("|") if es_alias else []
        n_en_alias += len(en_aliases)
        n_es_alias += len(es_aliases)

        for lang, main_norm, aliases, seen in (("en", en_norm, en_aliases, en_seen),
                                               ("es", es_norm, es_aliases, es_seen)):
            main_word = en if lang == "en" else es
            if main_norm is not None:
                seen.setdefault(main_norm, []).append((rid, main_word, "word"))
            own = set()
            for alias in aliases:
                a_norm = check_word(alias, lang, "alias")
                if a_norm is None:
                    continue
                if a_norm == main_norm:
                    err("%s: %s alias %r equals its own main word %r after normalisation" % (tag, lang, alias, main_word))
                    continue
                if a_norm in own:
                    err("%s: %s alias %r is repeated within the same row" % (tag, lang, alias))
                    continue
                own.add(a_norm)
                seen.setdefault(a_norm, []).append((rid, alias, "alias"))

        if en_norm is not None:
            if prev_key is not None and en_norm <= prev_key:
                err("%s: English word %r is not sorted after %r" % (tag, en, prev_en))
            prev_key, prev_en = en_norm, en

    def describe(entries):
        return ", ".join("id %s %s %r" % (rid, kind, word) for rid, word, kind in entries)

    for lang, seen in (("English", en_seen), ("Spanish", es_seen)):
        for norm in sorted(seen):
            entries = seen[norm]
            if len(entries) > 1:
                err("duplicate within %s (%r): %s" % (lang, norm, describe(entries)))

    for norm in sorted(set(en_seen) & set(es_seen)):
        en_ids = {rid for rid, _, _ in en_seen[norm]}
        es_ids = {rid for rid, _, _ in es_seen[norm]}
        if en_ids != es_ids:
            err("cross-language collision (%r): English %s <-> Spanish %s"
                % (norm, describe(en_seen[norm]), describe(es_seen[norm])))

    for message in errors:
        print("VIOLATION: " + message)
    print("checked %s" % path)
    print("rows: %d (expected %d); English aliases: %d; Spanish aliases: %d; "
          "distinct English strings: %d; distinct Spanish strings: %d"
          % (len(rows), EXPECTED_ROWS, n_en_alias, n_es_alias, len(en_seen), len(es_seen)))
    if errors:
        print("FAILED: %d violation(s)" % len(errors))
        return 1
    print("OK: no violations")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
