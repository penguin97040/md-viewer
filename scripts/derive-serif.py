#!/usr/bin/env python3
"""Derive the bold and italic faces of the reader's serif from Geist Serif.

Geist Serif ships one weight and no italic, and GPUI synthesises neither. These faces give the
serif setting real bold and italic in the window and in PDFs. Run with the path to an unmodified
GeistSerif-Regular.ttf: needs fonttools and skia-pathops. Output goes to assets/fonts.
"""
import sys
from pathlib import Path

import pathops
from fontTools.pens.transformPen import TransformPen
from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.ttLib import TTFont

BOLD_STROKE = 34      # font units added across each stem
SHEAR = 0.19          # about 11 degrees
ANGLE = -10.8
FAMILY = 'MDV Serif'
STYLES = {'Bold': (True, False), 'Italic': (False, True), 'Bold Italic': (True, True)}
HINTS = ('fpgm', 'prep', 'cvt ', 'hdmx', 'VDMX', 'LTSH', 'DSIG')


def outline(glyph_set, name, bold):
    path = pathops.Path()
    glyph_set[name].draw(path.getPen(glyphSet=glyph_set))
    if bold and len(path) > 0:
        thick = pathops.Path(path)
        thick.stroke(BOLD_STROKE, pathops.LineCap.BUTT_CAP, pathops.LineJoin.MITER_JOIN, 4)
        thick.convertConicsToQuads()
        # Each operand keeps its own fill. Merging the contours as one path makes the stroke's
        # winding cancel the glyph's, which leaves the letters hollow.
        path = pathops.op(path, thick, pathops.PathOp.UNION)
    return path


def derive(source, out_dir, style):
    bold, italic = STYLES[style]
    font = TTFont(source, recalcBBoxes=True, recalcTimestamp=False)
    glyph_set = font.getGlyphSet()
    x_height = font['OS/2'].sxHeight
    grow = BOLD_STROKE if bold else 0
    shift = (grow / 2 - (x_height / 2 * SHEAR if italic else 0))
    matrix = (1, 0, SHEAR if italic else 0, 1, shift, 0)
    glyphs = {}
    for name in font.getGlyphOrder():
        pen = TTGlyphPen(None)
        outline(glyph_set, name, bold).draw(TransformPen(pen, matrix))
        glyphs[name] = pen.glyph()
    glyf, hmtx = font['glyf'], font['hmtx']
    for name, glyph in glyphs.items():
        glyf[name] = glyph
        glyph.recalcBounds(glyf)
        width = hmtx[name][0]
        hmtx[name] = (width + grow if width else 0, getattr(glyph, 'xMin', 0) or 0)
    for tag in HINTS:
        if tag in font:
            del font[tag]
    os2, head, post = font['OS/2'], font['head'], font['post']
    os2.usWeightClass = 700 if bold else 400
    selection = os2.fsSelection & ~0b1100001
    selection |= (0b100000 if bold else 0) | (0b1 if italic else 0)
    os2.fsSelection = selection
    head.macStyle = (1 if bold else 0) | (2 if italic else 0)
    post.italicAngle = ANGLE if italic else 0
    if italic and 'hhea' in font:
        font['hhea'].caretSlopeRise, font['hhea'].caretSlopeRun = 1000, round(1000 * SHEAR)
    compact = style.replace(' ', '')
    names = font['name']
    for record in list(names.names):
        if record.nameID in (16, 17, 21, 22, 25) or record.nameID >= 256:
            names.removeNames(record.nameID)
    credit = 'Bold and italic derived from Geist Serif by scripts/derive-serif.py'
    for name_id, value in {1: FAMILY, 2: style, 3: f'{FAMILY} {style} derived', 4: f'{FAMILY} {style}',
                           6: f'MDVSerif-{compact}', 10: credit}.items():
        names.setName(value, name_id, 3, 1, 0x409)
        names.setName(value, name_id, 1, 0, 0)
    font.save(out_dir / f'MDVSerif-{compact}.ttf')


def main():
    root = Path(__file__).resolve().parent.parent
    for style in STYLES:
        derive(sys.argv[1], root / 'assets/fonts', style)


if __name__ == '__main__':
    main()
