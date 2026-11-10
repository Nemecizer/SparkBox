---
id: settings-appearance
title: "Settings: Appearance"
type: feature
area: settings
summary: Theme, text size, reduced motion and higher contrast.
keywords: [dark mode, light mode, black, oled, font size, contrast, accessibility, theme, text size, reduce motion]
verified: null
related: [arrange-your-windows, settings-window]
---

# Settings: Appearance

## What it's for

**Appearance** sets how SparkBox looks: one of three looks (light, dark, or black for an OLED screen or a dark room), how big the text is, and whether it moves less and shows stronger lines. The **Preview** at the top shows each change as you make it.

## Where to find it

- **Sidebar:** click **Settings**, then click **Appearance**.
- **Menu bar:** choose **SparkBox › Settings…**, then click **Appearance**.
- **Keyboard:** press <kbd>⌘,</kbd>, then click **Appearance**.
- **Command palette:** press <kbd>⌘K</kbd> and type `theme`, `text size`, `reduce motion` or `increase contrast`. The **Appearance** group sets each one directly.
- **Link:** [Open Appearance settings](open:settings?section=appearance)


## What's on the screen

### Preview

A small sample of SparkBox, with a title, buttons, two meters and some text. The two meters show the chosen theme's colors: in **Black** the GPU meter takes its brighter GPU color, while the memory meter stays amber because it's past its warning line. It's a picture, so nothing in it can be clicked.

### Theme and text

| Setting | Choices | Default | What it does |
|---|---|---|---|
| **Theme** | **System**, **Light**, **Dark**, **Black** | **System** | **System** follows macOS between **Light** and **Dark**, and the note below says which it's using now. It never picks **Black**. **Black** is for an OLED screen or a dark room: a true black desk with the windows barely lifted off it, and brighter colors for the charts, meters and status marks. The numbers stay white. The window's title bar and menus switch too; in **Black** they use the dark look, and so do Help's pictures |
| **Text size** | **Small**, **Default**, **Large**, **Larger** | **Default** | Changes the text in every window, tables included |

To make everything bigger, not only the text, use **View › Zoom In**.

### Accessibility

| Setting | Choices | Default | What it does |
|---|---|---|---|
| **Reduce motion** | **System**, **On** | **System** | **On** stops pulsing, spinning and sliding |
| **Increase contrast** | **System**, **On** | **System** | **On** gives darker lines, a solid selection and a thicker focus ring |

**System** follows **System Settings › Accessibility › Display** in macOS.

## What it changes

These settings change only how SparkBox looks on this Mac, and they need no unlock. They take effect at once. To undo one, choose the previous setting.

They're kept in SparkBox's window storage on this Mac. The Terminal has its own theme and font in [Settings: Terminal](help:settings-terminal).

## Related How-Tos

Each setting here takes effect as soon as you choose it, and the **Preview** shows the result. To change where things sit on the desk, not how they look:

- [Arrange your windows](help:arrange-your-windows)
