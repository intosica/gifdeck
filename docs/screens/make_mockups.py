#!/usr/bin/env python3
"""Draws the simulated clock screenshots used in the README (no real screen capture, no personal data).
Usage: python3 make_mockups.py && inkscape -w 900 clocks-analog.svg -o clocks-analog.png (see README)."""
import math

BLUE, RED, GREY, FG = "#3584e4", "#e01b24", "#9a9996", "#2e3436"
CARD = "#ffffff"; BG = "#fafafa"; FONT = "Cantarell, Noto Sans, sans-serif"

def pt(cx, cy, r, m):  # minute 0..720 -> point, 12 o'clock = 0, clockwise
    a = m / 720 * 2 * math.pi
    return cx + r * math.sin(a), cy - r * math.cos(a)

def arc(cx, cy, r, a, b):
    (x1, y1), (x2, y2) = pt(cx, cy, r, a), pt(cx, cy, r, b)
    return f"M {x1:.1f} {y1:.1f} A {r} {r} 0 {int(b - a > 360)} 1 {x2:.1f} {y2:.1f}"

def dial(cx, cy, title, own, others, color, labels=("12", "3", "6", "9"), caption="", pm=False):
    s = f'<text x="{cx}" y="{cy-98}" text-anchor="middle" font-size="15" font-weight="bold" fill="{FG}">{title}</text>'
    s += f'<circle cx="{cx}" cy="{cy}" r="70" fill="{FG}" fill-opacity=".06" stroke="{FG}" stroke-opacity=".35"/>'
    for i in range(12):
        (x1, y1), (x2, y2) = pt(cx, cy, 70 - (9 if i % 3 == 0 else 5), i * 60), pt(cx, cy, 70, i * 60)
        s += f'<line x1="{x1:.1f}" y1="{y1:.1f}" x2="{x2:.1f}" y2="{y2:.1f}" stroke="{FG}" stroke-opacity=".5"/>'
    for k, t in enumerate(labels):
        x, y = pt(cx, cy, 52, k * 180)
        s += f'<text x="{x:.1f}" y="{y+5:.1f}" text-anchor="middle" font-size="13" fill="{FG}">{t}</text>'
    for a, b, c in others:
        s += f'<path d="{arc(cx, cy, 80, a, b)}" stroke="{c}" stroke-width="7" fill="none" opacity=".85"/>'
    a, b = own
    x1, y1 = pt(cx, cy, 70, a)
    s += f'<path d="M {cx} {cy} L {x1:.1f} {y1:.1f} A 70 70 0 {int(b - a > 360)} 1 {pt(cx, cy, 70, b)[0]:.1f} {pt(cx, cy, 70, b)[1]:.1f} Z" fill="{color}" fill-opacity=".28"/>'
    s += f'<path d="{arc(cx, cy, 68.5, a, b)}" stroke="{color}" stroke-width="4" fill="none"/>'
    for dx, t, on in ((-52, "AM", not pm), (-2, "PM", pm)):
        s += f'<rect x="{cx+dx}" y="{cy+88}" width="50" height="26" rx="6" fill="{BLUE if on else FG}" fill-opacity="{.9 if on else .1}"/><text x="{cx+dx+25}" y="{cy+106}" text-anchor="middle" font-size="13" fill="{"#fff" if on else FG}">{t}</text>'
    s += f'<text x="{cx}" y="{cy+136}" text-anchor="middle" font-size="14" font-weight="bold" fill="{color}">{caption}</text>'
    return s

def svg(w, h, body):
    return (f'<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}" font-family="{FONT}">'
            f'<rect width="{w}" height="{h}" rx="14" fill="{BG}"/>{body}</svg>')

def card(x, y, w, h):
    return f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="12" fill="{CARD}" stroke="{FG}" stroke-opacity=".15"/>'

# --- analog: "Morning" 9:00-12:00 is alone (blue), "Work" 11:00-5:00 PM overlaps it
def analog():
    b = f'<text x="450" y="38" text-anchor="middle" font-size="19" font-weight="bold" fill="{FG}">Analog clock: press, hold and sweep</text>'
    b += card(30, 60, 400, 340) + card(470, 60, 400, 340)
    # left: rule A being edited. B overlaps -> grey/red outside
    b += dial(230, 200, "Morning (9:00 AM – 11:30 AM)", (540, 690), [(660, 720, RED)], RED, caption="overlaps “Work”")
    b += dial(670, 200, "Work (11:00 AM – 5:00 PM)", (660, 720), [(540, 690, RED)], RED, caption="overlaps “Morning”")
    b += f'<text x="450" y="425" text-anchor="middle" font-size="14" fill="{FG}">Overlapping rules show the other one as a ring and both turn red. Without an overlap: grey ring, blue arc.</text>'
    return svg(900, 450, b)

def analog_ok():
    b = f'<text x="450" y="38" text-anchor="middle" font-size="19" font-weight="bold" fill="{FG}">Analog clock: no clash (grey ring = another rule of the same preset)</text>'
    b += card(30, 60, 400, 340) + card(470, 60, 400, 340)
    b += dial(230, 200, "Morning (9:00 AM – 11:30 AM)", (540, 690), [(600, 720, GREY)], BLUE, caption="overlaps only its own preset")
    b += dial(670, 200, "Evening (6:00 PM – 10:00 PM)", (360, 600), [], BLUE, caption="free", pm=True)
    return svg(900, 450, b)

def digital():
    def t(x, y, text, c, hl_min=False):
        s = ""
        if hl_min:
            ox = x + (text.index(":") + 1) * 24
            s += f'<rect x="{ox-4}" y="{y-34}" width="56" height="46" rx="10" fill="{c}" fill-opacity=".2"/>'
        return s + f'<text x="{x}" y="{y}" font-size="40" font-weight="bold" fill="{c}" font-family="monospace" xml:space="preserve">{text}</text>'
    def row(y, name, a, b, c, note, hl=False):
        s = card(30, y, 840, 110) + f'<text x="56" y="{y+32}" font-size="16" font-weight="bold" fill="{FG}">{name}</text>'
        s += t(56, y+80, a, c, hl) + f'<text x="330" y="{y+76}" font-size="34" fill="{FG}">–</text>' + t(380, y+80, b, c)
        return s + f'<text x="660" y="{y+76}" font-size="15" fill="{c}">{note}</text>'
    body = f'<text x="450" y="38" text-anchor="middle" font-size="19" font-weight="bold" fill="{FG}">Digital clock: click the numbers, drag up or down</text>'
    body += row(60, "Morning", " 8:30 AM", " 9:30 AM", BLUE, "blue: no clash", True)
    body += row(185, "Work", "10:00 AM", " 5:00 PM", RED, "red: overlaps another schedule")
    body += row(310, "Evening", " 4:30 PM", "10:00 PM", RED, "red: overlaps “Work”")
    return svg(900, 440, body)

for name, f in (("clocks-analog-overlap", analog), ("clocks-analog-free", analog_ok), ("clocks-digital", digital)):
    open(f"{name}.svg", "w").write(f())
