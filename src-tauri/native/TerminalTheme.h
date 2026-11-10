#pragma once

// User-specified Ghostty settings, mapped to the public libghostty-vt API and AppKit renderer.
static const CGFloat BrigTerminalFontSize = 14;
static const CGFloat BrigTerminalCellHeightScale = 1.5;
static const CGFloat BrigTerminalPaddingX = 10;
static const CGFloat BrigTerminalPaddingY = 10;
static const uint32_t BrigTerminalForeground = 0xffe735;
static const uint32_t BrigTerminalBackground = 0x121212;
static const uint32_t BrigTerminalCursor = 0xaeb9c4;
static const uint32_t BrigTerminalSelectionBackground = 0xaeb9c4;
static const uint32_t BrigTerminalSelectionForeground = 0x000000;
static const uint32_t BrigTerminalPalette[16] = {
    0x1d2021, 0xd4380d, 0x52c41a, 0xffc069, 0x4f86cd, 0xd3adf7, 0x597ef7, 0xbfcad5,
    0x7c6f64, 0xff9c6e, 0xb8bb26, 0xffe735, 0x83a598, 0xd3869b, 0x8ec07c, 0xffffff,
};
static inline GhosttyColorRgb brigTerminalRGB(uint32_t value) {
    return (GhosttyColorRgb){(value >> 16) & 0xff, (value >> 8) & 0xff, value & 0xff};
}
