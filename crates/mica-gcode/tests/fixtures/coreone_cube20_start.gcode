; Start G-code PrusaSlicer 2.9.6 produced for tests/models/cube20.stl centred at (125,110), Prusa CORE One HF0.4,
; 0.20mm SPEED, Prusament PLA. Template: start_gcode of mica-profiles/tests/fixtures/coreone_hf04_speed_pla.ini
; (PrusaSlicer's AGPL-3.0 PrusaResearch.ini 2.4.14 with the 2.5.12 firmware version, see that header).
; Lines below are compared verbatim, blank lines removed.
M17 ; enable steppers
M862.1 P0.4 A0 F1 ; nozzle check
M862.3 P "COREONE" ; printer model check
M862.5 P2 ; g-code level check
M862.6 P"Input shaper" ; FW feature check
M115 U6.8.1+16182
M555 X115 Y96 W32 H24
G90 ; use absolute coordinates
M83 ; extruder relative mode
M140 S60 ; set bed temp
M109 R170 ; wait for temp
M84 E ; turn off E motor
G28 ; home all without mesh bed level
M141 S20 ; set nominal chamber temp
M106 S70
G0 Z40 F10000
M104 T0 S170
M190 R60 ; wait for bed temp
M107
G29 G ; absorb heat
M109 R170 ; wait for MBL temp
M302 S155 ; lower cold extrusion limit to 155C
G1 E-2 F2400 ; retraction
M84 E ; turn off E motor
G29 P9 X208 Y-2.5 W32 H4
;
; MBL
;
M84 E ; turn off E motor
G29 P1 ; invalidate mbl & probe print area
G29 P1 X150 Y0 W100 H20 C ; probe near purge place
G29 P3.2 ; interpolate mbl probes
G29 P3.13 ; extrapolate mbl outside probe area
G29 A ; activate mbl
; prepare for purge
M104 S230
G0 X249 Y-2.5 Z15 F4800 ; move away and ready for the purge
M109 S230
G92 E0
M569 S0 E ; set spreadcycle mode for extruder
M591 S0 ; disable stuck detection
;
; Extrude purge line
;
G92 E0 ; reset extruder position
G1 E2 F2400 ; deretraction after the initial one
G0 E5 X235 Z0.2 F500 ; purge
G0 X225 E4 F500 ; purge
G0 X215 E4 F650 ; purge
G0 X205 E4 F800 ; purge
G0 X202 Z0.05 F8000 ; wipe, move close to the bed
G0 X199 Z0.2 F8000 ; wipe, move quickly away from the bed
M591 R ; restore stuck detection
G92 E0
M221 S100 ; set flow to 100%
G21 ; set units to millimeters
G90 ; use absolute coordinates
M83 ; use relative distances for extrusion
