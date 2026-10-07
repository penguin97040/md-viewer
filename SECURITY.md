# Security

## Reporting a vulnerability

Please report security problems privately through GitHub: open the
repository's **Security** tab and choose **Report a vulnerability**. Please
don't open a public issue for them.

Fixes go into the latest release only.

## What a markdown file can and can't do

A markdown file is plain text and can't run anything by itself, but a viewer
that shows it as a web page has to be careful. MD Viewer treats every file as
untrusted:

- **No script.** The HTML in a document is sanitised, and the page's
  Content-Security-Policy only runs the viewer's own scripts. Diagrams
  (mermaid) and maths (KaTeX) run in their strictest modes, and a document
  can't change their settings.
- **No contact with the internet unless you agree.** The page can only load
  from the app itself. Images from the web are held back, and a bar offers to
  load them for that file, because loading one tells its site that you
  opened the file, and from where. When you agree, they are fetched by the
  app, not the page.
- **Links open outside.** Web and email links open in your browser. Links to
  other markdown files open in a new tab. Nothing else is followed, and the
  viewer itself never navigates away.
- **Local files.** A document can show image files from your computer, as
  images, and nothing else. It can't reach files on other computers: on
  Windows, opening `\\server\share\...` would send that server a hash of
  your password. The exception is a share that an open document is itself
  on, so documents on a network drive still show their pictures.
- **Limits.** Files over 256 MB aren't opened. Syntax highlighting that runs
  for over a second is skipped, so a crafted code block can't hang the app.

Some crafted files can still take a while to open, for example a single
paragraph of hundreds of thousands of `*` and `_` markers. They slow the app
down but can't do harm.
