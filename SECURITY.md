# Security

## Reporting a vulnerability

Please report security problems privately through GitHub: open the
repository's **Security** tab and choose **Report a vulnerability**. Please
don't open a public issue for them.

Fixes go into the latest release only.

## What a markdown file can and can't do

A markdown file is plain text and can't run anything by itself, but a viewer
has to be careful with what it shows. MD Viewer treats every file as
untrusted:

- **No script.** There is no web view and no script engine. Raw HTML in a
  document is read for a fixed set of tags and anything else becomes text or
  disappears, so nothing in a document can execute. Diagrams (Mermaid) and
  maths are drawn by renderers built into the app. A document can't change
  their settings: `%%{...}%%` directives are dropped, and so are `click`
  lines in flowcharts, class diagrams and state diagrams. What they produce is
  checked against an allow-list before it is drawn, which refuses scripts,
  event attributes, external links and fonts, and external `url()` references.
- **No contact with the internet unless you agree.** Images from the web are
  held back, and a bar offers to load them for that tab, because loading one
  tells its site that you opened the file, and from where. Nothing is
  requested until you choose **Load images**. When you agree, the app fetches
  them, and this is the only network connection the app makes for a document.
  Saving a PDF fetches nothing.
  - Destinations must resolve only to public IP addresses. Every address a
    name resolves to is checked when the connection is made, and the app
    connects only to an address it checked, so changing a DNS answer
    afterwards doesn't help. Redirects are followed one at a time and checked
    each time. Private, loopback, link-local, shared, reserved and
    documentation addresses are blocked, along with IPv6 translation and
    tunnel ranges that could reach an internal IPv4 address.
  - Environment proxies are not used. No cookies or referrer are sent.
    Addresses with a user name or password are refused.
  - Only images are accepted: the response must be an image type and the
    bytes must decode as PNG, JPEG, GIF, WebP, BMP, ICO or SVG. A fetch is
    limited to 25 MB, 5 redirects and 30 seconds, and a closed document's
    fetches stop.
  - Certificates are checked against the host name using the Mozilla root list
    built into the app, not your operating system's store. A TLS-inspecting
    network, such as a company proxy with its own root certificate, isn't
    trusted, so web images fail there. Nothing else is affected.
  - Fetched images are kept in memory only, for up to an hour, and are gone
    when the app exits.
- **Links open outside.** Web (`http`, `https`) and email links open with the
  system's default app. Links to other markdown files open in a new tab.
  Nothing else is followed; other kinds of link keep their text and lose their
  target. In a PDF, links to other files are dropped.
- **Local files.** A document can show image files from your computer, as
  images, and nothing else: the file must have an image extension, be a
  regular file of at most 25 MB, and decode as an allowed image format. It
  can't reach files on other computers: on Windows, opening
  `\\server\share\...` would send that server a hash of your password. These
  paths are refused in every spelling the app knows of, on every platform. The
  exception is a share that an open document is itself on, so documents on a
  network drive still show their pictures. A symbolic link or junction that
  already exists beside the document is followed by the file system, so one
  pointing at a share would still connect.
- **SVG images** can't reach outside their file: links and embedded images
  resolve to nothing and DTDs are refused. Files that would expand into more
  than a million elements or 1,024 levels are refused, and memory use is
  bounded.
- **Limits.** Files over 256 MB aren't opened. Syntax highlighting that runs
  for over a second is skipped, so a crafted code block can't hang the app.
  Decoded images are capped at 400 MB each and 1 GB in all. Diagrams and maths
  have time limits (5 s and 2 s), and a render that overruns can't block
  later ones.
- **One document can't take the others down.** If layout, drawing, input or
  search for a document fails with a panic, only that tab fails and shows
  "This file could not be shown. Other tabs are not affected."
- **Single-instance channel.** Later launches pass their file paths to the
  running app through a channel private to your user account (a named pipe
  with an owner-only descriptor on Windows, a socket with mode 0600 in a
  private directory elsewhere). It only carries paths to open.
- **No test switches.** The environment switches used for testing exist only
  in builds made with the non-default `dev-harness` feature, not in releases.

## What isn't covered

- **Crashes.** A stack overflow or a failed memory allocation ends the whole
  app in Rust, so these are prevented with limits, not caught. A panic in the
  app's own interface code (the window, tabs and settings, as opposed to a
  document) also closes the app. It shows a message box on Windows and a line
  on standard error elsewhere, naming only the code location; nothing is saved
  or sent.
- **Slow files.** Some crafted files can still take a while to open, for
  example a single paragraph of hundreds of thousands of `*` and `_` markers.
  They slow the app down but can't do harm. A diagram render that overruns its
  limit keeps using a CPU core until it ends, because a thread can't be
  stopped.
- **Fonts.** Maths drawing and PDF output may read a font file already on your
  computer for characters outside the built-in fonts. Nothing in a document
  chooses which file is read.
- **Platforms.** Most of the checks above were run on Linux. The Windows and
  macOS code was compiled for those systems but has had little real use, and
  macOS is newer and less tested. Treat the platform-specific protections
  (network paths, the launch channel) as less proven there.
