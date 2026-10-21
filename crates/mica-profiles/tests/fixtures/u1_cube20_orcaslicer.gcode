; Reference: OrcaSlicer 2.4.1 macOS CLI, 2026-10-15; installed Snapmaker vendor presets.
; Printer: Snapmaker U1 (0.4 nozzle); process: 0.20 Standard @Snapmaker U1 (0.4 nozzle).
; Filament: Snapmaker PLA @U1 in four slots, only T0 used; curr_bed_type: High Temp Plate.
; Input: tests/models/cube20.stl; --arrange 1 --slice 0, flattened preset JSON, isolated --datadir.
; Blank lines retained; M73 progress removed. No geometry, thumbnails or vendor profiles included.
; Reproduction and version notes: docs/u1-klipper.md. Includes preamble after custom start.
; ---- start ----
;TYPE:Custom
SET_PRINT_AUTO_BED_LEVELING ENABLE=1
SET_TIME_LAPSE_CAMERA ENABLE=1
;===== date: 20251222 =====================

PRINT_START
DEFECT_DETECTION_START
SET_PRINT_STATS_INFO TOTAL_LAYER=100
SET_PRINT_STATS_INFO CURRENT_LAYER=0
TIMELAPSE_START
M140 S55
M104 T0 S140
M204 S10000

G28 X Y
;===== 床面异物检测 ========
T0
G90
DEFECT_DETECTION_DETECT_BED
;===== 取放头检测 =================
SM_PRINT_CHECK_SWITCH_EXTRUDER

;===== 自动进料 & 挤出流量 & 预挤出 ======================
SM_PRINT_EXTRUDER_PREHEAT EXTRUDER=1 TEMP=140
SM_PRINT_AUTO_FEED EXTRUDER=0
SM_PRINT_FLOW_CALIBRATE EXTRUDER=0
SM_PRINT_EXTRUDER_PREHEAT EXTRUDER=2 TEMP=140
SM_PRINT_AUTO_FEED EXTRUDER=1
SM_PRINT_FLOW_CALIBRATE EXTRUDER=1
SM_PRINT_EXTRUDER_PREHEAT EXTRUDER=3 TEMP=140
SM_PRINT_AUTO_FEED EXTRUDER=2
SM_PRINT_FLOW_CALIBRATE EXTRUDER=2
SM_PRINT_AUTO_FEED EXTRUDER=3
SM_PRINT_FLOW_CALIBRATE EXTRUDER=3
M104 S0 T0 A0
M104 S0 T1 A0
M104 S0 T2 A0
M104 S0 T3 A0
M104 T0 S130

;===== 粗回零 =================
T0
M106 S255
M106 P2 S0
MOVE_TO_DISCARD_FILAMENT_POSITION
M109 T0 S130
ROUGHLY_CLEAN_NOZZLE_WITH_DISCARD
MOVE_TO_XY_IDLE_POSITION_EXTRUDER
G28 Z I140 J140

;===== 检测钢板 =================
DETECT_BED_PLATE

;===== 深度清洁喷嘴 =================
G90
G0 Z5 F10000
MOVE_TO_DISCARD_FILAMENT_POSITION
M109 S170
ROUGHLY_CLEAN_NOZZLE
MOVE_TO_XY_IDLE_POSITION_EXTRUDER
FINELY_CLEAN_NOZZLE_STAGE_1
M104 S130
G0 Z5 F10000
MOVE_TO_DISCARD_FILAMENT_POSITION
ROUGHLY_CLEAN_NOZZLE
MOVE_TO_XY_IDLE_POSITION_EXTRUDER
FINELY_CLEAN_NOZZLE_STAGE_2

;===== 精回零 =================
M106 S255
M109 S130
M190 S55
M107 P2
G90
G0 Z5 F10000
G28 Z

;===== 热床调平 =================
; Always pass `ADAPTIVE_MARGIN=0` because Orca has already handled `adaptive_bed_mesh_margin` internally
; Make sure to set ADAPTIVE to 0 otherwise Klipper will use it's own adaptive bed mesh logic
BED_MESH_CALIBRATE mesh_min=123.5,124 mesh_max=147.5,148 ALGORITHM=bicubic PROBE_COUNT=4,4 ADAPTIVE=0 ADAPTIVE_MARGIN=0
; Original upstream: BED_MESH_CALIBRATE PROBE_COUNT=11,11

;===== 画起始线 =================
G90
G1 Z1.5
G0 X10 Y3 Z2 F18000
M109 S220
G1 Z0.2
M83
G1 X110 E15 F360
G1 Z1.5

G90
M106 S0
G90
G21
M83 ; use relative distances for extrusion
SET_PRESSURE_ADVANCE ADVANCE=0.02; Override pressure advance value
T0
M106 S0
M106 P2 S0
; ---- end ----
;===== date: 20260605 =====================
; layer 99
; max_layer_z 20.05
; print_sequence by layer



PRINT_END
TIMELAPSE_STOP
