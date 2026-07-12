#!/usr/bin/env python3

"""生成由面对面观察者组成的 Sponge schematic v3 压力场景."""

import argparse
import gzip
import logging
import os
from pathlib import Path
import struct
from typing import BinaryIO


DATA_VERSION = 4790
DEFAULT_SIZE = 100
DEFAULT_OUTPUT = Path("assets/schematics/observer-clock-100.schem")

TAG_END = 0
TAG_SHORT = 2
TAG_INT = 3
TAG_BYTE_ARRAY = 7
TAG_LIST = 9
TAG_COMPOUND = 10
TAG_INT_ARRAY = 11


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="生成一个立方体观察者面对面高频时钟原理图.",
    )
    parser.add_argument(
        "--size",
        type=int,
        default=DEFAULT_SIZE,
        help="立方体边长, 必须是正偶数, 默认 100.",
    )
    parser.add_argument(
        "--output",
        type=Path,
        default=DEFAULT_OUTPUT,
        help="输出 Sponge schematic v3 文件路径.",
    )
    return parser.parse_args()


def validate_size(size: int) -> int:
    if size <= 0 or size % 2 != 0:
        raise ValueError("size 必须是正偶数, 以便沿 X 轴完整配对观察者")
    if size > 32_767:
        raise ValueError("size 不能超过 Sponge schematic 的有符号 short 范围")
    volume = size**3
    if volume > 2_147_483_647:
        raise ValueError("方块数量超过 NBT byte array 的长度范围")
    return volume


def write_i16(output: BinaryIO, value: int) -> None:
    output.write(struct.pack(">h", value))


def write_i32(output: BinaryIO, value: int) -> None:
    output.write(struct.pack(">i", value))


def write_string_payload(output: BinaryIO, value: str) -> None:
    encoded = value.encode("utf-8")
    output.write(struct.pack(">H", len(encoded)))
    output.write(encoded)


def write_named_header(output: BinaryIO, tag: int, name: str) -> None:
    output.write(bytes([tag]))
    write_string_payload(output, name)


def write_named_int(output: BinaryIO, name: str, value: int) -> None:
    write_named_header(output, TAG_INT, name)
    write_i32(output, value)


def write_named_short(output: BinaryIO, name: str, value: int) -> None:
    write_named_header(output, TAG_SHORT, name)
    write_i16(output, value)


def write_named_int_array(output: BinaryIO, name: str, values: tuple[int, ...]) -> None:
    write_named_header(output, TAG_INT_ARRAY, name)
    write_i32(output, len(values))
    for value in values:
        write_i32(output, value)


def write_empty_list(output: BinaryIO, name: str) -> None:
    write_named_header(output, TAG_LIST, name)
    output.write(bytes([TAG_END]))
    write_i32(output, 0)


def write_palette(output: BinaryIO) -> None:
    write_named_header(output, TAG_COMPOUND, "Palette")
    write_named_int(output, "minecraft:air", 0)
    write_named_int(
        output,
        "minecraft:observer[facing=east,powered=false]",
        1,
    )
    output.write(bytes([TAG_END]))


def write_block_data(output: BinaryIO, size: int, volume: int) -> None:
    write_named_header(output, TAG_BYTE_ARRAY, "Data")
    write_i32(output, volume)
    row = bytes([1, 0]) * (size // 2)
    layer = row * size
    progress_interval = max(1, size // 10)
    for y in range(size):
        output.write(layer)
        completed = y + 1
        if completed % progress_interval == 0 or completed == size:
            logging.info("方块数据写入进度: %d/%d 层", completed, size)


def write_schematic(output: BinaryIO, size: int, volume: int) -> None:
    write_named_header(output, TAG_COMPOUND, "")
    write_named_header(output, TAG_COMPOUND, "Schematic")
    write_named_int(output, "Version", 3)
    write_named_int(output, "DataVersion", DATA_VERSION)
    write_named_short(output, "Width", size)
    write_named_short(output, "Height", size)
    write_named_short(output, "Length", size)
    write_named_int_array(output, "Offset", (0, 0, 0))

    write_named_header(output, TAG_COMPOUND, "Blocks")
    write_palette(output)
    write_block_data(output, size, volume)
    write_empty_list(output, "BlockEntities")
    output.write(bytes([TAG_END]))

    write_empty_list(output, "Entities")
    output.write(bytes([TAG_END]))
    output.write(bytes([TAG_END]))


def generate(size: int, output: Path) -> None:
    volume = validate_size(size)
    output.parent.mkdir(parents=True, exist_ok=True)
    temporary = output.with_name(f".{output.name}.tmp")
    logging.info(
        "开始生成 %dx%dx%d 单侧原理图, 包含 %d 个观察者, 镜像后形成 %d 对时钟",
        size,
        size,
        size,
        volume // 2,
        volume // 2,
    )
    try:
        with temporary.open("wb") as raw:
            with gzip.GzipFile(
                filename="",
                mode="wb",
                fileobj=raw,
                compresslevel=6,
                mtime=0,
            ) as compressed:
                write_schematic(compressed, size, volume)
        os.replace(temporary, output)
    except Exception:
        temporary.unlink(missing_ok=True)
        raise
    logging.info("原理图已写入 %s, 压缩后 %d bytes", output, output.stat().st_size)


def main() -> None:
    logging.basicConfig(level=logging.INFO, format="%(levelname)s %(message)s")
    args = parse_args()
    generate(args.size, args.output)


if __name__ == "__main__":
    main()
