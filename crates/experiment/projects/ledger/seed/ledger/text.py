"""Laying out text reports: aligned columns."""

LEFT = "left"
RIGHT = "right"

# The space between two columns.
GAP = "  "


def widths(rows):
    """The width of each column: its longest cell."""
    result = []
    for row in rows:
        for i, cell in enumerate(row):
            if i == len(result):
                result.append(0)
            result[i] = max(result[i], len(cell))
    return result


def pad(cell, width, align):
    if align == RIGHT:
        return cell.rjust(width)
    return cell.ljust(width)


def table(rows, aligns):
    """`rows`, lists of strings, as aligned text lines, each column aligned as `aligns` says (LEFT
    or RIGHT, one per column). Trailing spaces are dropped from each line."""
    sizes = widths(rows)
    lines = []
    for row in rows:
        cells = [pad(cell, sizes[i], aligns[i]) for i, cell in enumerate(row)]
        lines.append(GAP.join(cells).rstrip())
    return "".join(line + "\n" for line in lines)


def rule(width, char="-"):
    """A horizontal rule `width` characters long."""
    return char * width
