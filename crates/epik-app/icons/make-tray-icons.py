"""Draws the tray icons: Epik's four-node mark, redrawn for 18 px.

The mark shrunk to tray size is unreadable, so the nodes are larger and
the edges thicker. Each icon is drawn at 1x (18 px) and 2x (36 px), eight
times larger and then scaled down for smooth edges.

- macOS: templates, black on transparent, which the menu bar tints for
  light and dark. Idle has hollow nodes; building has solid ones.
- Windows and Linux: in color. Idle is gray; building has the mark's
  colors.

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


def draw(size, node_colors, edge_color, hollow):
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
    return image.resize((size, size), Image.LANCZOS)


ICONS = {
    "idle-template": ([BLACK] * 4, EDGE_BLACK, True),
    "building-template": ([BLACK] * 4, EDGE_BLACK, False),
    "idle": ([GRAY] * 4, EDGE_GRAY, False),
    "building": ([GREEN, GRAY, GRAY, GREEN], EDGE_GRAY, False),
}

for name, (nodes, edges, hollow) in ICONS.items():
    for suffix, size in (("", 18), ("@2x", 36)):
        draw(size, nodes, edges, hollow).save(f"{name}{suffix}.png")
