"""Draws the tray icons: Epik's four-node mark, redrawn for 18 px.

The mark shrunk to tray size is unreadable, so the nodes are larger and
the edges thicker. Each icon is drawn at 1x (18 px) and 2x (36 px), eight
times larger and then scaled down for smooth edges.

- macOS: templates, black on transparent, which the menu bar tints for
  light and dark. Idle has hollow nodes; building has solid ones.
- Windows and Linux: in color. Idle is gray; building has the mark's
  colors.
- Needing attention: a red dot at the top right. On Windows and Linux it
  is added to the idle and building icons. On macOS a template can't be
  red, so the "-gray" icons draw the template marks in a gray that reads
  on light and dark menu bars, with the dot.

Run it from this directory with Pillow installed: python3 make-tray-icons.py
"""

from PIL import Image, ImageDraw

SUPER = 8
# Node centers and radii, and edges, in a 64-unit square.
NODES = [((14, 14), 11), ((50, 22), 9.5), ((22, 50), 9.5), ((43, 49), 8.5)]
EDGES = [(0, 1), (0, 2), (1, 3), (2, 3)]
EDGE_WIDTH = 4.5
RING_WIDTH = 4.5

BLACK = (0, 0, 0, 255)
EDGE_BLACK = (0, 0, 0, 150)
GRAY = (140, 140, 140, 255)
EDGE_GRAY = (140, 140, 140, 150)
GREEN = (0, 192, 122, 255)
RED = (215, 58, 74, 255)
# The dot's center and radius, and the clear gap around it.
DOT = ((55.5, 8.5), 8.5)
DOT_GAP = 2.5


def draw(size, node_colors, edge_color, hollow, dot):
    big = size * SUPER
    scale = big / 64
    image = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    pen = ImageDraw.Draw(image)
    for a, b in EDGES:
        (ax, ay), _ = NODES[a]
        (bx, by), _ = NODES[b]
        pen.line([(ax * scale, ay * scale), (bx * scale, by * scale)], fill=edge_color, width=round(EDGE_WIDTH * scale))
    for ((x, y), r), color in zip(NODES, node_colors):
        box = [(x - r) * scale, (y - r) * scale, (x + r) * scale, (y + r) * scale]
        if hollow:
            # Clear the edges under the ring's hole, so the node reads as hollow.
            pen.ellipse(box, fill=(0, 0, 0, 0))
            pen.ellipse(box, outline=color, width=round(RING_WIDTH * scale))
        else:
            pen.ellipse(box, fill=color)
    if dot:
        (x, y), r = DOT
        gap = r + DOT_GAP
        # Clear around the dot, so it stands apart from the mark.
        pen.ellipse([(x - gap) * scale, (y - gap) * scale, (x + gap) * scale, (y + gap) * scale], fill=(0, 0, 0, 0))
        pen.ellipse([(x - r) * scale, (y - r) * scale, (x + r) * scale, (y + r) * scale], fill=RED)
    return image.resize((size, size), Image.LANCZOS)


ICONS = {
    "idle-template": ([BLACK] * 4, EDGE_BLACK, True, False),
    "building-template": ([BLACK] * 4, EDGE_BLACK, False, False),
    "idle": ([GRAY] * 4, EDGE_GRAY, False, False),
    "building": ([GREEN, GRAY, GRAY, GREEN], EDGE_GRAY, False, False),
    "idle-attention": ([GRAY] * 4, EDGE_GRAY, False, True),
    "building-attention": ([GREEN, GRAY, GRAY, GREEN], EDGE_GRAY, False, True),
    "idle-attention-gray": ([GRAY] * 4, EDGE_GRAY, True, True),
    "building-attention-gray": ([GRAY] * 4, EDGE_GRAY, False, True),
}

for name, (nodes, edges, hollow, dot) in ICONS.items():
    for suffix, size in (("", 18), ("@2x", 36)):
        draw(size, nodes, edges, hollow, dot).save(f"{name}{suffix}.png")
