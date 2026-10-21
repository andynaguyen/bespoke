Bespoke's Annotate records someone's screen while they talk, click and draw red marks on it. They send a capture to show you a problem instead of describing it, so the capture is the bug report. Read it closely before you answer, and don't ask them to explain again what it already shows.

A pasted capture starts with a line like `[scrawl 0:33 · App “Window” · /path/to/scrawl-1792432139176-1a2b-3.scrawl]`, followed by its keyframe lines. The files live on the Mac that runs Bespoke. If that path doesn't exist on this machine, pass it to `get_capture` as `capture`. Without a paste, `latest` is the newest capture, and `list_captures` shows the others.

How to read a capture:

1. `get_capture` returns the summary and the contact sheet. The sheet shows every keyframe in time order, left to right and top to bottom, each with a `#N M:SS` badge.
2. Each `#N (M:SS)` line is a keyframe: what they did in brackets, then what they said while it was on screen. A line with no quote has the action alone. Coordinates are fractions of the captured area from the top left, so x 0.51 is just right of center.
3. Speech recognition mishears names and code terms, so trust the screen when the two disagree.
4. Call `get_frames` for keyframes you need at full size. When the summary has a `Cropped to:` line, pass `full: true` to see the whole screen.
5. `get_manifest` returns `scrawl.json`: marks with their shape and box, click and key events, word timings, and how each keyframe was cropped.

Red strokes, circles, underlines and arrows are their marks, and they point at the problem. A ripple around the cursor shows a click. A glowing red trail that tapers off is the laser pointer, used to point without leaving a mark.

When you answer, refer to moments as "#3 (0:10)" so they can find them on the contact sheet.
