# SPDX-License-Identifier: AGPL-3.0-only
# Provenance: original
#
# dmgbuild settings for the Mica Slicer disk image, used by scripts/release.sh:
#   dmgbuild -s settings.py -D app=<Mica Slicer.app> -D dir=<this folder> "Mica Slicer" out.dmg
# They write the Finder window layout (.DS_Store) directly: no Finder scripting.

# The background design: variant-1 (minimal arrow), variant-2 (dotted curve), variant-3
# (layer bars) or variant-4 (layer lines with a solid arc). Each is variant-N.svg, rendered
# to variant-N.png and variant-N@2x.png by render.sh; dmgbuild combines the pair into one
# Retina-ready TIFF.
background_variant = "variant-4"

import os.path  # noqa: E402

_dir = defines["dir"]  # noqa: F821 (dmgbuild provides `defines`)
_app = defines["app"]  # noqa: F821
_app_name = os.path.basename(_app)

format = "UDZO"
filesystem = "HFS+"
files = [_app]
symlinks = {"Applications": "/Applications"}
icon = os.path.join(_dir, "../../../assets/icon/mica.icns")
background = os.path.join(_dir, background_variant + ".png")

# The artwork is 640 x 420 pt (plus a 20 pt bleed at the bottom); the window frame adds the
# title bar (32 pt on macOS 26, 28 pt before).
window_rect = ((200, 120), (640, 452))
default_view = "icon-view"
show_toolbar = False
show_tab_view = False
show_sidebar = False
show_status_bar = False
show_pathbar = False
show_icon_preview = False
include_icon_view_settings = True
include_list_view_settings = False
arrange_by = None
grid_spacing = 100
label_pos = "bottom"
text_size = 13
icon_size = 128
# Icon centers in the artwork's coordinates; the labels land on its plates at y 258.
icon_locations = {_app_name: (160, 175), "Applications": (480, 175)}
hide_extensions = [_app_name]
