# Reference sources — READ ONLY, NEVER COPY

This directory holds third-party source trees kept **purely as behavioral reference**.
Nothing here is tracked by git and nothing here ships with Relay.

## Why this is not in the repository

Relay's public repository must stay free of GPL source. `reference/` is listed in
`.gitignore`; obtain the trees locally with the commands below.

## FileZilla 3.5.3 — GPL-2.0-or-later

```sh
git clone https://github.com/basvodde/filezilla.git reference/filezilla
git -C reference/filezilla checkout 8f874dfe2fdfee4a95cf706bc860252a5793c32e   # 2012-01-20
```

**License: GPL-2.0-or-later.** Copying any of this code — verbatim, translated to Rust,
or line-by-line adapted — into Relay would make Relay a derivative work and force the
entire application under the GPL. Do not do it.

What you *may* do: read it to understand observed behavior, wire formats, and protocol
edge cases, then implement them independently. Documented formats and observable
behavior are not copyrightable; expression is.

### What it is actually useful for

| Path | Why |
|---|---|
| `src/engine/directorylistingparser.cpp` | The catalog of real-world `LIST` formats (Unix, DOS/IIS, EPLF, VMS, MVS, OS/9, z/VM, HP NonStop, MLSD) and their date/size quirks. |
| `src/engine/ftpcontrolsocket.cpp` | FTP state machine, PASV/EPSV fallbacks, TLS session-reuse handling. |
| `src/engine/*.h` (public API) | The command-in / notification-out API shape that `relay-core` mirrors. |
| `src/engine/servercapabilities.*` | Per-server capability cache (FEAT/MLSD/MFMT) and timezone-offset inference. |

See `ROADMAP.md` §1 for the full analysis of why the engine is re-implemented rather
than reused.
