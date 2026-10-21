; PrusaSlicer 2.9.6 output for tests/models/cube20.stl centred at (180,180): printer 'Original Prusa XL - 5T Input Shaper 0.4 nozzle',
; print '0.20mm SPEED @XLIS 0.4', filament 'Prusament PLA @XLIS', vendor profiles 2.5.11, whose custom G-code for this selection is identical in PrusaSlicer's AGPL-3.0 PrusaResearch.ini (2.4.14). Only the custom G-code before the first
; layer and after the last layer is kept (layer moves removed).
;TYPE:Custom
M17 ; enable steppers
M862.3 P "XL" ; printer model check
M862.5 P2 ; g-code level check
M862.6 P"Input shaper" ; FW feature check
M115 U6.4.1+12026
G90 ; use absolute coordinates
M83 ; extruder relative mode
; set print area
M555 X170 Y170 W20 H20
; inform about nozzle diameter
M862.1 T0 P0.4 A0 F0
; turn off unused heaters
M104 T1 S0
M104 T2 S0
M104 T3 S0
M104 T4 S0
M217 Z2 ; set toolchange z hop to 2mm, or zhop variable from slicer if higher
; set bed and extruder temp for MBL
M140 S60 ; set bed temp
M104 T0 S170 ; set temp
; Home XY
G28 XY
; try picking tools used in print
G1 F24000
; select tool that will be used to home & MBL
T0 S1 L0 D0
M109 T0 S170 ; wait for temp
; home Z with MBL tool
M84 E ; turn off E motor
G28 Z
G0 Z5 ; add Z clearance
M104 T0 S70 ; set idle temp
M190 S60 ; wait for bed temp
G29 G ; absorb heat
M109 T0 S170 ; wait for temp
; move to the nozzle cleanup area
G1 X202 Y163 Z5 F24000
M302 S155 ; lower cold extrusion limit to 155C
G1 E-2 F2400 ; retraction for nozzle cleanup
; nozzle cleanup
M84 E ; turn off E motor
G29 P9 X170 Y163 W32 H7
G0 Z5 F480 ; move away in Z
M107 ; turn off the fan
; MBL
M84 E ; turn off E motor
G29 P1 ; invalidate mbl & probe print area
G29 P1 X30 Y0 W50 H20 C ; probe near purge place
G29 P3.2 ; interpolate mbl probes
G29 P3.13 ; extrapolate mbl outside probe area
G29 A ; activate mbl
G1 Z10 F720 ; move away in Z
G1 F24000
P0 S1 L1 D0; park the tool
; set extruder temp
M104 T0 S230
;
; purge initial tool
;
G1 F24000
P0 S1 L2 D0; park the tool
M109 T0 S230
T0 S1 L0 D0; pick the tool
G92 E0 ; reset extruder position
G0 X30 Y-7 Z10 F24000 ; move close to the sheet's edge
G0 E30 X40 Z0.2 F170 ; purge while moving towards the sheet
G0 X70 E9 F800 ; continue purging and wipe the nozzle
G0 X73 Z0.05 F8000 ; wipe, move close to the bed
G0 X76 Z0.2 F8000 ; wipe, move quickly away from the bed
G1 E-0.8 F2400 ; retract
M591 R ; restore stuck detection
G92 E0 ; reset extruder position
G21 ; set units to millimeters
G90 ; use absolute coordinates
M83 ; use relative distances for extrusion
M572 S0.036 ; Pressure advance
M142 S36 ; set heatbreak target temp
M107
; ---- end ----
; Filament-specific end gcode
; Filament-specific end gcode
; Filament-specific end gcode
; Filament-specific end gcode
; Filament-specific end gcode
G4 ; wait
G1 Z25 ; Move bed down
P0 S1 ; park tool
G1 Z117 F300 ; Move bed further down
; turn off extruder heaters
M104 T0 S0
M140 S0 ; turn off heatbed
M107 ; turn off fan
M221 S100 ; reset flow percentage
M84 ; disable motors
M77 ; stop print timer
; max_layer_z = 20
