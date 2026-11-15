# GeoZip security notes

GeoZip is **experimental**. Nobody outside the project has audited it. Use it
to try the idea and to review it, and keep another copy of anything important.

## What it protects

The contents of your files and, with "hide file names", their names, against
someone who obtains the `.zip` file but does not know your places (or your
words).

The building blocks are standard. Nothing cryptographic was invented here:

- **Argon2id** turns the secret into a key. It is slow and memory-hungry on
  purpose.
- **AES-256** in WinZip AE-2 mode (AES-CTR + HMAC-SHA1) encrypts the data.
- A **random salt per archive** means the same places give a different key in
  every archive, and no attack can be prepared once and reused against
  everyone.

## How much it withstands

It depends on who is attacking. Each 300 m cell is 1 in about 5.7 billion
(32 bits), but people do not pick places at random. The two "realistic" columns
are estimates, not measurements.

| Level | On paper | Stranger | Someone who knows you | Guaranteed floor |
|---|---|---|---|---|
| High (5 places) | 162 bits | 75–120 bits | 30–60 bits | 0 |
| Extreme (5 places + 1 word each) | 216 bits | 119–174 bits | 74–114 bits | 44–54 bits |
| Extreme with two words each | 256 (key size) | 168–223 bits | 123–163 bits | 93–103 bits |

- **Guaranteed floor**: what is left if the attacker knows your exact places
  and their order. At the Extreme level that is the words, chosen at random by
  the program: 1,850 possibilities per place (10.9 bits) with one word, or
  1,710,325 pairs (20.7 bits) with two. Asking for a different word costs at
  most 2 bits for that place (three changes are allowed), which is where the
  range in each figure comes from.
- A floor of 54 bits is about 700,000 years with 1,000 computers, or about 700
  years with a million. It only matters against someone who knows your exact
  places and has resources on that scale; the two-word option is there for that
  case.
- With Argon2id at about one second per guess, 60 bits is tens of millions of
  years with 1,000 computers; 30 bits is about 12 days.

**Do not rely on the High level against someone who knows you well and has
resources**, or if you used the online map and are worried about the map
provider (see below). Use Extreme in those cases.

## Known risks

| Risk | Detail |
|---|---|
| Guessable places | Your home, your workplace and famous landmarks are the first things an attacker would try. Choose a specific spot inside a memory, not the obvious location. |
| The map server | Clicks never leave the program, but the server sees which areas you download (tiles about 2 km across) and in what order. To avoid that, work offline with a `.pmtiles` map or type coordinates. |
| Online search | "Search streets online" sends the text you typed to an external server, which then knows the spot almost exactly. Searching for cities and towns is local and sends nothing. |
| Coordinates copied from a website | If you take them from a map in a browser, that service sees where you looked. |
| Forgetting | There is no recovery. Forgetting a place, the order or a word means losing the contents. |
| A compromised computer | Spyware on your machine sees your clicks and keystrokes. No file encryption protects against that. |
| Visible names | Without "hide file names", the names, sizes and dates of the files can be read without the key. |
| Integrity of the set | With visible names, someone who can modify the file can delete, reorder or rename entries undetected. They cannot read or alter the contents. |
| Deleting without the key | For the same reason, GeoZip itself lets you delete entries from an archive with visible names without entering the places. |
| Remembered secret | After you open or unlock a protected archive, the places and words stay in memory until you close it, so they are not asked for on every operation. They are never written to disk. |
| Memory | With "hide file names", the whole contents pass through RAM when creating and opening. Secrets are not exhaustively wiped from memory. |
| Word list not frozen | The English/Spanish list still needs review by a native speaker. If it changes before 1.0, Extreme-level archives created earlier will stop opening. |

## What to review if you audit it

1. `crates/geozip-core/src/grid.rs` — the grid (section 2 of `SPEC.md`).
2. `crates/geozip-core/src/secret.rs` — canonical string, Argon2id and word
   generation.
3. `crates/geozip-core/src/header.rs` — header parsing and limits.
4. `crates/geozip-core/src/archive.rs` — writing and reading the ZIP, rejection
   of unencrypted entries, safe extraction paths, in-place editing.
5. `crates/geozip-core/tests/archive.rs` — includes the test in which 7-Zip
   opens an archive with the derived key.

## Reporting a problem

Open an issue with the component, the steps to reproduce, what you expected,
what happened, and the impact you see.

If the problem would let someone decrypt other people's archives, please report
it privately first, through
[GitHub's private vulnerability reporting](https://github.com/colocoquillo/GeoZip/security/advisories/new),
so there is time to fix it before it is public.
