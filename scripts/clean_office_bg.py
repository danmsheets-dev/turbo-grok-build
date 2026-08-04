"""Clean game mode office_bg.png: uniform floor, remove door, stray pixels."""
from __future__ import annotations

from pathlib import Path

import numpy as np
from PIL import Image

PATH = Path("crates/codegen/xai-grok-pager/assets/game_mode/office_bg.png")

FLOOR_A = np.array([61, 154, 159], dtype=np.float32)
FLOOR_B = np.array([56, 143, 150], dtype=np.float32)
FLOOR_HI = np.array([70, 165, 170], dtype=np.float32)
FLOOR_LO = np.array([48, 130, 136], dtype=np.float32)


def floor_color(x: int, y: int, tile: int = 16) -> np.ndarray:
    tx = x % tile
    ty = y % tile
    tile_i = ((x // tile) + (y // tile)) % 2
    base = FLOOR_A if tile_i == 0 else FLOOR_B
    if tx == 0 or ty == 0:
        return FLOOR_HI
    if tx == tile - 1 or ty == tile - 1:
        return FLOOR_LO
    if (tx + ty) % 5 == 0:
        return FLOOR_LO * 0.85 + base * 0.15
    if (tx * 3 + ty) % 7 == 0:
        return FLOOR_HI * 0.5 + base * 0.5
    return base


def is_teal_floor(rgb: np.ndarray) -> bool:
    r, g, b = float(rgb[0]), float(rgb[1]), float(rgb[2])
    if g < 100 or b < 100:
        return False
    if r > 100:
        return False
    if g > b + 40:
        return False
    return True


def is_rug(rgb: np.ndarray) -> bool:
    r, g, b = float(rgb[0]), float(rgb[1]), float(rgb[2])
    return r > 70 and r > g + 15 and r > b + 15 and g < 90 and b < 90


def is_door_brown(rgb: np.ndarray) -> bool:
    r, g, b = float(rgb[0]), float(rgb[1]), float(rgb[2])
    return r > 55 and g < 70 and b < 55 and r > g + 15 and r > b + 20


def is_black_bar(rgb: np.ndarray) -> bool:
    return float(rgb[0]) + float(rgb[1]) + float(rgb[2]) < 25


def put_floor(out: np.ndarray, x: int, y: int) -> None:
    c = floor_color(x, y)
    out[y, x, 0] = int(c[0])
    out[y, x, 1] = int(c[1])
    out[y, x, 2] = int(c[2])
    out[y, x, 3] = 255


def main() -> None:
    im = Image.open(PATH).convert("RGBA")
    arr = np.array(im)
    h, w = arr.shape[:2]
    out = arr.copy()

    y_floor0 = int(h * 0.40)
    y_floor1 = h - 8
    n_floor = n_door = n_stray = 0

    for y in range(y_floor0, y_floor1):
        for x in range(w):
            rgb = out[y, x, :3]
            a = out[y, x, 3]
            if a < 200 or is_black_bar(rgb):
                continue
            if is_rug(rgb):
                continue

            # Door (bottom-right wood) → floor
            if x > int(w * 0.78) and y > int(h * 0.68) and is_door_brown(rgb):
                put_floor(out, x, y)
                n_door += 1
                continue

            # Door frame / hardware greys in door zone
            if x > int(w * 0.78) and y > int(h * 0.72):
                r, g, b = float(rgb[0]), float(rgb[1]), float(rgb[2])
                if abs(r - g) < 25 and abs(g - b) < 25 and 35 < r < 170 and not is_teal_floor(
                    rgb
                ):
                    put_floor(out, x, y)
                    n_door += 1
                    continue

            if is_teal_floor(rgb):
                put_floor(out, x, y)
                n_floor += 1

    # Stray dark/brown speckles on left mid floor
    for y in range(int(h * 0.48), int(h * 0.70)):
        for x in range(int(w * 0.02), int(w * 0.25)):
            rgb = out[y, x, :3]
            if is_black_bar(rgb) or is_rug(rgb):
                continue
            r, g, b = float(rgb[0]), float(rgb[1]), float(rgb[2])
            br = (r + g + b) / 3
            if (br < 50 and not is_teal_floor(rgb)) or (r > 40 and g < 40 and b < 35):
                put_floor(out, x, y)
                n_stray += 1

    # Remaining door browns
    for y in range(int(h * 0.70), y_floor1):
        for x in range(int(w * 0.80), w - 4):
            rgb = out[y, x, :3]
            if is_black_bar(rgb):
                continue
            r, g, b = map(float, rgb)
            if is_door_brown(rgb) or (r > 70 and g < 60 and b < 55):
                put_floor(out, x, y)
                n_door += 1

    Image.fromarray(out, "RGBA").save(PATH, "PNG", optimize=True)
    print(f"rewrote floor={n_floor} door={n_door} stray={n_stray} -> {PATH} size={w}x{h}")


if __name__ == "__main__":
    main()
