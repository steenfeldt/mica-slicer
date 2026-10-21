// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 tests/libslic3r/test_placeholder_parser.cpp
// Changes: Catch2 sections become Rust tests; the variables come from a hand-built Context instead of
// Changes: DynamicPrintConfig::full_print_config(). The FloatOrPercent / *_extrusion_width cases
// Changes: (perimeter_extrusion_width, first_layer_extrusion_width, support_material_xy_spacing,
// Changes: external_perimeter_speed, infill_overlap, first_layer_speed) are left out: they test
// Changes: PrusaSlicer's print config dependency resolution, which is not part of this crate.

use mica_template::{Context, Value, ValueType, Var, evaluate_boolean, process};

/// Variables of the "Placeholder parser scripting" scenario.
fn scripting() -> Context {
    let mut ctx = Context::with_seed(1);
    ctx.set(
        "printer_notes",
        "  PRINTER_VENDOR_PRUSA3D  PRINTER_MODEL_MK2  ",
    );
    ctx.set("nozzle_diameter", Var::floats([0.6, 0.6, 0.6, 0.6]));
    ctx.set("temperature", Var::ints([357, 359, 363, 378]));
    ctx.set(
        "filament_retract_length",
        Var::nullable(
            ValueType::Float,
            [Some(Value::Float(5.0)), None, Some(Value::Float(3.0))],
        ),
    );
    ctx.set("foo", 0);
    ctx.set("bar", 2);
    ctx.set("num_extruders", 4);
    // An enum option; the template language sees enums as strings.
    ctx.set("gcode_flavor", "marlin");
    ctx
}

fn p(template: &str) -> String {
    process(template, &mut scripting()).unwrap_or_else(|e| panic!("{template}: {e}"))
}

fn pf(template: &str) -> f64 {
    p(template).parse().unwrap()
}

fn b(expression: &str) -> bool {
    evaluate_boolean(expression, &mut scripting()).unwrap_or_else(|e| panic!("{expression}: {e}"))
}

fn approx(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-5 * (1.0 + b.abs())
}

#[test]
fn legacy_and_basic_expansion() {
    assert_eq!(p("[temperature_[foo]]"), "357");
    assert_eq!(p("{temperature[foo]}"), "357");
    assert_eq!(p("test [ temperature_ [foo] ] \n hu"), "test 357 \n hu");
    assert_eq!(p("{is_nil(filament_retract_length[0])}"), "false");
    assert_eq!(p("{is_nil(filament_retract_length[1])}"), "true");
    assert_eq!(p("{is_nil(filament_retract_length[2])}"), "false");
    assert_eq!(p("{temperature[foo];temperature[foo]}"), "357357");
    assert_eq!(p("{temperature[foo];;;temperature[foo];}"), "357357");
    assert_eq!(p("{temperature[foo];;temperature[foo];}"), "357357");
    assert_eq!(p("{temperature[foo];;;temperature[foo];;}"), "357357");
    assert_eq!(p(r#"{"hu\nha\\\"ha\""}"#), "hu\nha\\\"ha\"");
}

#[test]
fn utf8() {
    // Full-width plus sign instead of a plain + inside a code block.
    assert!(process("{1\u{FF0B} 3}", &mut scripting()).is_err());
    // Japanese "cool" inside a string literal and outside of code blocks.
    assert_eq!(p("{1+\"かっこいい\"+\" \"+3}"), "1かっこいい 3");
    assert_eq!(p("{1+3}かっこいい"), "4かっこいい");
}

#[test]
fn math() {
    assert_eq!(p("{2*3}"), "6");
    assert_eq!(p("{2*3/6}"), "1");
    assert_eq!(p("{2*3/12}"), "0");
    assert!(approx(pf("{2.*3/12}"), 0.5));
    assert!(approx(pf("{10%2.5}"), 0.0));
    assert!(approx(pf("{11%2.5}"), 1.0));
    assert_eq!(p("{2*(3-12)}"), "-18");
    assert_eq!(p("{2*foo*(3-12)}"), "0");
    assert_eq!(p("{2*bar*(3-12)}"), "-36");
    assert!(approx(pf("{2.5*bar*(3-12)}"), -45.0));
    assert_eq!(p("{min(12, 14)}"), "12");
    assert_eq!(p("{max(12, 14)}"), "14");
    assert!(approx(pf("{min(13.4, -1238.1)}"), -1238.1));
    assert!(approx(pf("{max(13.4, -1238.1)}"), 13.4));
    assert_eq!(p("{int(13.4)}"), "13");
    assert_eq!(p("{int(-13.4)}"), "-13");
    assert_eq!(p("{round(13.4)}"), "13");
    assert_eq!(p("{round(-13.4)}"), "-13");
    assert_eq!(p("{round(13.6)}"), "14");
    assert_eq!(p("{round(-13.6)}"), "-14");
}

#[test]
fn digits() {
    assert_eq!(p("{digits(5, 15)}"), "              5");
    assert_eq!(p("{digits(5., 15)}"), "              5");
    assert_eq!(p("{zdigits(5, 15)}"), "000000000000005");
    assert_eq!(p("{zdigits(5., 15)}"), "000000000000005");
    assert_eq!(p("{digits(5, 15, 8)}"), "     5.00000000");
    assert_eq!(p("{digits(5., 15, 8)}"), "     5.00000000");
    assert_eq!(p("{zdigits(5, 15, 8)}"), "000005.00000000");
    assert_eq!(p("{zdigits(5., 15, 8)}"), "000005.00000000");
    assert_eq!(p("{digits(13.84375892476, 15, 8)}"), "    13.84375892");
    assert_eq!(p("{zdigits(13.84375892476, 15, 8)}"), "000013.84375892");
}

#[test]
fn ternary() {
    assert_eq!(p("{12 == 12 ? 1 - 3 : 2 * 2 * unknown_symbol}"), "-2");
    assert_eq!(p("{12 == 21/2 ? 1 - 1 - unknown_symbol : 2 * 2}"), "4");
    assert_eq!(p("{12 == 13 ? 1 - 1 * unknown_symbol : 2 * 2}"), "4");
    assert_eq!(p("{12 == 2 * 6 ? 1 - 1 : 2 * unknown_symbol}"), "0");
    assert_eq!(
        p(
            "{12 == 2 * 6 ? 3 - 1 != 2 ? does_not_exist : 0 * 0 - 0 / 1 + 12345 : bull ? 3 - cokoo : 2 * unknown_symbol}"
        ),
        "12345"
    );
}

#[test]
fn interpolate_table() {
    assert!(approx(
        pf("{interpolate_table(13.84375892476, (0, 0), (20, 20))}"),
        13.84375892476
    ));
    assert!(approx(
        pf("{interpolate_table(13, (0, 0), (20, 20), (30, 20))}"),
        13.0
    ));
    assert!(approx(
        pf("{interpolate_table(25, (0, 0), (20, 20), (30, 20))}"),
        20.0
    ));
}

#[test]
fn boolean_expressions() {
    assert!(b("12 == 12"));
    assert!(!b("12 != 12"));
    assert!(b("\"has some PATTERN embedded\" =~ /.*PATTERN.*/"));
    assert!(!b("\"has some PATTERN embedded\" =~ /.*PTRN.*/"));
    assert!(b("foo + 2 == bar"));
    assert!(!b("foo + 3 == bar"));
    assert!(b("(12 == 12) and (13 != 14)"));
    assert!(b("(12 == 12) && (13 != 14)"));
    assert!(b("(12 == 12) or (13 == 14)"));
    assert!(b("(12 == 12) || (13 == 14)"));
    assert!(b("(12 == 12) and not (13 == 14)"));
    assert!(b(
        "(12 == 12) ? (1 - 1 == 0) : (2 * 2 == 3 * unknown_symbol)"
    ));
    assert!(!b(
        "(12 == 21/2) ? (1 - 1 == 0 - unknown_symbol) : (2 * 2 == 3)"
    ));
    assert!(b(
        "(12 == 13) ? (1 - 1 == 3 * unknown_symbol) : (2 * 2 == 4)"
    ));
    assert!(!b(
        "(12 == 2 * 6) ? (1 - 1 == 3) : (2 * 2 == 4 * unknown_symbol)"
    ));
    assert!(!b("12 < 3"));
    assert!(b("12 < 22"));
    assert!(b("12 > 3"));
    assert!(!b("12 > 22"));
    assert!(!b("12 <= 3"));
    assert!(b("12 <= 22"));
    assert!(b("12 >= 3"));
    assert!(!b("12 >= 22"));
    assert!(b("12 <= 12"));
    assert!(b("12 >= 12"));
}

#[test]
fn one_of() {
    assert!(b(r#"one_of("a", "a", "b", "c")"#));
    assert!(b(r#"one_of("b", "a", "b", "c")"#));
    assert!(b(r#"one_of("c", "a", "b", "c")"#));
    assert!(!b(r#"one_of("d", "a", "b", "c")"#));
    assert!(!b(r#"one_of("a")"#));
    assert!(b(r#"one_of("a", "a")"#));
    assert!(!b(r#"one_of("b", "a")"#));
    assert!(b(r#"one_of("abcdef", /.*c.*/)"#));
    assert!(b(r#"one_of("abcdef", /.*f.*/, /.*c.*/)"#));
    assert!(b(r#"one_of("abcdef", ~".*f.*", ~".*c.*")"#));
    assert!(!b(r#"one_of("ghij", /.*f.*/, /.*c.*/)"#));
    assert!(!b(r#"one_of("ghij", ~".*f.*", ~".*c.*")"#));
}

#[test]
fn complex_boolean_expressions() {
    assert!(b(
        "printer_notes=~/.*PRINTER_VENDOR_PRUSA3D.*/ and printer_notes=~/.*PRINTER_MODEL_MK2.*/ and nozzle_diameter[0]==0.6 and num_extruders>1"
    ));
    assert!(b(
        "printer_notes=~/.*PRINTER_VEwerfNDOR_PRUSA3D.*/ or printer_notes=~/.*PRINTertER_MODEL_MK2.*/ or (nozzle_diameter[0]==0.6 and num_extruders>1)"
    ));
    assert!(!b(
        "printer_notes=~/.*PRINTER_VEwerfNDOR_PRUSA3D.*/ or printer_notes=~/.*PRINTertER_MODEL_MK2.*/ or (nozzle_diameter[0]==0.3 and num_extruders>1)"
    ));
    assert!(b("gcode_flavor == \"marlin\""));
}

#[test]
fn write_to_output_variables() {
    let mut ctx = scripting();
    ctx.set_output("writable_string", "");
    process("{writable_string = \"Written\"}", &mut ctx).unwrap();
    assert_eq!(process("{writable_string}", &mut ctx).unwrap(), "Written");

    let mut ctx = scripting();
    ctx.set_output("writable_floats", Var::floats([0.0, 0.0, 0.0]));
    process("{writable_floats[1] = 33}", &mut ctx).unwrap();
    assert_eq!(
        ctx.output("writable_floats"),
        Some(&Var::floats([0.0, 33.0, 0.0]))
    );
}

// ---- "Placeholder parser variables" scenario ----

/// The `config` passed as config_override in the upstream tests.
fn variables() -> Context {
    let mut ctx = Context::with_seed(2);
    ctx.set("filament_notes", Var::strings(["testnotes"]));
    ctx.set("enable_dynamic_fan_speeds", Var::bools([true]));
    ctx.set("nozzle_diameter", Var::floats([0.6, 0.6, 0.6, 0.6]));
    ctx.set("temperature", Var::ints([357, 359, 363, 378]));
    ctx
}

fn v(template: &str) -> String {
    process(template, &mut variables()).unwrap_or_else(|e| panic!("{template}: {e}"))
}

#[test]
fn create_scalar_variables() {
    for scope in ["local", "global"] {
        let t = |s: &str| v(&s.replace("SCOPE", scope));
        assert_eq!(t("{SCOPE myint = 33+2}{myint}"), "35");
        assert_eq!(
            t("{SCOPE mystr = \"mine\" + \"only\" + \"mine\"}{mystr}"),
            "mineonlymine"
        );
        assert_eq!(t("{SCOPE mybool = 1 + 1 == 2}{mybool}"), "true");

        assert_eq!(t("{SCOPE myint = 33+2}{myint = 12}{myint}"), "12");
        assert_eq!(
            t("{SCOPE mystr = \"mine\" + \"only\" + \"mine\"}{mystr = \"yours\"}{mystr}"),
            "yours"
        );
        assert_eq!(
            t("{SCOPE mybool = 1 + 1 == 2}{mybool = false}{mybool}"),
            "false"
        );

        assert_eq!(t("{SCOPE myint = 33+2}{SCOPE myint = 12}{myint}"), "12");
        assert_eq!(
            t("{SCOPE mystr = \"mine\" + \"only\" + \"mine\"}{SCOPE mystr = \"yours\"}{mystr}"),
            "yours"
        );
        assert_eq!(
            t("{SCOPE mybool = 1 + 1 == 2}{SCOPE mybool = false}{mybool}"),
            "false"
        );
    }
}

#[test]
fn create_vector_variables() {
    for scope in ["local", "global"] {
        let t = |s: &str| v(&s.replace("SCOPE", scope));
        assert_eq!(t("{SCOPE myint = repeat(2*3, 4*6)}{myint[5]}"), "24");
        assert_eq!(
            t("{SCOPE mystr = repeat(2*3, \"mine\" + \"only\" + \"mine\")}{mystr[5]}"),
            "mineonlymine"
        );
        assert_eq!(
            t("{SCOPE mybool = repeat(5, 1 + 1 == 2)}{mybool[4]}"),
            "true"
        );

        assert_eq!(t("{SCOPE myint = (2*3, 4*6, 5*5)}{myint[1]}"), "24");
        assert_eq!(
            t("{SCOPE mystr = (2*3, \"mine\" + \"only\" + \"mine\", 8)}{mystr[1]}"),
            "mineonlymine"
        );
        assert_eq!(
            t("{SCOPE mybool = (2*3 == 8, 1 + 1 == 2, 5*5 != 33)}{mybool[1]}"),
            "true"
        );

        assert_eq!(t("{SCOPE myint = temperature}{myint[0]}"), "357");
        assert_eq!(t("{SCOPE mystr = filament_notes}{mystr[0]}"), "testnotes");
        assert_eq!(
            t("{SCOPE mybool = enable_dynamic_fan_speeds}{mybool[0]}"),
            "true"
        );
    }
}

#[test]
fn copy_and_overwrite_vectors() {
    assert_eq!(
        v("{local myint = temperature}{myint = repeat(2*3, 4*6)}{myint[5]}"),
        "24"
    );
    assert_eq!(
        v("{local myint = temperature}{myint = (2*3, 4*6)}{myint[1]}"),
        "24"
    );
    assert_eq!(
        v("{local myint = temperature}{myint = (1)}{myint = temperature}{myint[0]}"),
        "357"
    );

    assert_eq!(
        v(
            "{local mystr = filament_notes}{mystr = repeat(2*3, \"mine\" + \"only\" + \"mine\")}{mystr[5]}"
        ),
        "mineonlymine"
    );
    assert_eq!(
        v(
            "{local mystr = filament_notes}{mystr = (2*3, \"mine\" + \"only\" + \"mine\")}{mystr[1]}"
        ),
        "mineonlymine"
    );
    assert_eq!(
        v(
            "{local mystr = filament_notes}{mystr = (2*3, \"mine\" + \"only\" + \"mine\")}{mystr = filament_notes}{mystr[0]}"
        ),
        "testnotes"
    );

    assert_eq!(
        v("{local mybool = enable_dynamic_fan_speeds}{mybool = repeat(2*3, true)}{mybool[5]}"),
        "true"
    );
    assert_eq!(
        v("{local mybool = enable_dynamic_fan_speeds}{mybool = (false, true)}{mybool[1]}"),
        "true"
    );
    assert_eq!(
        v(
            "{local mybool = enable_dynamic_fan_speeds}{mybool = (false, false)}{mybool = enable_dynamic_fan_speeds}{mybool[0]}"
        ),
        "true"
    );
}

#[test]
fn size_and_empty() {
    assert_eq!(v("{local myint = (0, 1, 2, 3)}{size(myint)}"), "4");
    assert_eq!(v("{local myint = (0);myint=();size(myint)}"), "0");
    assert_eq!(v("{local myint = (0, 1, 2, 3)}{empty(myint)}"), "false");
    assert_eq!(v("{local myint = (0);myint=();empty(myint)}"), "true");
}

#[test]
fn nested_if_with_new_variables() {
    assert_eq!(
        v(
            "{if 1 == 1}{local myints = (5, 4, 3, 2, 1)}{else}{local myfloats = (1., 2., 3., 4., 5., 6., 7.)}{endif}{myints[1]},{size(myints)}"
        ),
        "4,5"
    );
    assert_eq!(
        v(
            "{if 1 == 0}{local myints = (5, 4, 3, 2, 1)}{else}{local myfloats = (1., 2., 3., 4., 5., 6., 7.)}{endif}{size(myfloats)}"
        ),
        "7"
    );
    assert_eq!(
        v(
            "{if 1 == 0 then local myints = (5, 4, 3, 2, 1);else;local myfloats = (1., 2., 3., 4., 5., 6., 7.);endif}{size(myfloats)}"
        ),
        "7"
    );
    assert_eq!(
        v(
            "{if 1 == 1}{if 2 == 3}{nejaka / haluz}{else}{local myints = (6, 5, 4, 3, 2, 1)}{endif}{else}{if zase * haluz}{else}{local myfloats = (1., 2., 3., 4., 5., 6., 7.)}{endif}{endif}{size(myints)}"
        ),
        "6"
    );
    assert_eq!(
        v(
            "{if false then else;local myfloats = (1., 2., 3., 4., 5., 6., 7.);endif}{size(myfloats)}"
        ),
        "7"
    );
    assert_eq!(
        v(
            "{if 1 == 1 then if 2 == 3}nejaka / haluz{else local myints = (6, 5, 4, 3, 2, 1) endif else if zase * haluz then else local myfloats = (1., 2., 3., 4., 5., 6., 7.) endif endif}{size(myints)}"
        ),
        "6"
    );
    assert_eq!(
        v(
            "{if 1 == 1 then if 2 == 3 then nejaka / haluz else}{local myints = (6, 5, 4, 3, 2, 1)}{endif else if zase * haluz then else local myfloats = (1., 2., 3., 4., 5., 6., 7.) endif endif}{size(myints)}"
        ),
        "6"
    );
    assert_eq!(
        v(
            "{if 1 == 1 then if 2 == 3 then nejaka / haluz else}{local myints = (6, 5, 4, 3, 2, 1)}{endif else}{if zase * haluz}{else local myfloats = (1., 2., 3., 4., 5., 6., 7.) endif}{endif}{size(myints)}"
        ),
        "6"
    );
    assert_eq!(v("{if false then elsif false then else endif}"), "");
}
