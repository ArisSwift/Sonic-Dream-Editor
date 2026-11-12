# Interface assets

Files that `ui.xml` names with a relative path live here. The XML is embedded
in the binary, so at startup the editor points Raven's base path at this folder
(and at `assets/` beside the executable once the binary is moved elsewhere).

    <Image Source="icons/xp.png" Slot.Size="18,18"/>

PNG and SVG both work (`Rtexture_create_from_file` decodes PNG, JPEG, BMP, TGA,
GIF, PSD, HDR, PIC and PNM; `Rtexture_create_from_svg` takes SVG). Keep icons
square and even-sized, and give them twice the pixels of their displayed size
on a HiDPI screen.
