// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/PlaceholderParser.cpp :: client::macro_processor
// Changes: Tests written for Mica that pin down PrusaSlicer behaviour read from the C++ grammar and
// Changes: semantic actions (type promotion, conversions, scopes, error cases and parse quirks).

use mica_template::{Context, RandomSource, Value, ValueType, Var, evaluate_boolean, process};

fn ctx() -> Context {
    let mut c = Context::with_seed(7);
    c.set("temperature", Var::ints([215, 220, 225]));
    c.set("nozzle_diameter", Var::floats([0.4, 0.6]));
    c.set("abrasive", Var::bools([false, true]));
    c.set("notes", "line one\nline \\two");
    c.set("names", Var::strings(["PLA", "PETG"]));
    c.set("idx", 1);
    c.set("height", 0.2);
    c.set("count", 3);
    c.set("enabled", true);
    c.set("nil_scalar", Value::Nil);
    c.set("empty_ints", Var::ints([]));
    c.set("empty_strings", Var::strings(Vec::<String>::new()));
    c.set(
        "maybe",
        Var::nullable(ValueType::Int, [None, Some(Value::Int(5))]),
    );
    c
}

fn p(template: &str) -> String {
    process(template, &mut ctx()).unwrap_or_else(|e| panic!("{template}: {e}"))
}

fn fails(template: &str) -> String {
    match process(template, &mut ctx()) {
        Ok(out) => panic!("{template}: expected an error, got {out:?}"),
        Err(e) => e.message,
    }
}

#[test]
fn arithmetic_promotion() {
    assert_eq!(p("{7/2}"), "3");
    assert_eq!(p("{-7/2}"), "-3");
    assert_eq!(p("{-7%2}"), "-1");
    assert_eq!(p("{7.0/2}"), "3.5");
    assert_eq!(p("{7/2.}"), "3.5");
    assert_eq!(p("{2*3.5}"), "7");
    assert_eq!(p("{1234567.0}"), "1.23457e+06");
    assert_eq!(p("{0.1+0.2}"), "0.3");
    assert_eq!(p("{-0.0}"), "-0");
    assert_eq!(p("{1e3}"), "1000");
    assert_eq!(p("{.5 + 5.}"), "5.5");
    assert_eq!(p("{height * 3}"), "0.6");
    assert_eq!(p("{min(2, 2.5)}"), "2");
    assert_eq!(p("{max(2, 2.5)}"), "2.5");
    assert_eq!(p("{+3}"), "3");
    assert_eq!(p("{- -3}"), "3");
    assert!(fails("{1/0}").contains("Division by zero"));
    assert!(fails("{1%0.0}").contains("Division by zero"));
    assert!(fails("{true + 1}").contains("Cannot add non-numeric types"));
    assert!(fails("{\"a\" - 1}").contains("Cannot subtract"));
    assert!(fails("{-true}").contains("unary minus"));
    assert!(fails("{not 1}").contains("not operator"));
}

#[test]
fn string_concatenation() {
    assert_eq!(p("{\"a\" + 1 + 2}"), "a12");
    assert_eq!(p("{1 + 2 + \"a\"}"), "3a");
    assert_eq!(p("{\"h=\" + height}"), "h=0.2");
    assert_eq!(p("{true + \"!\"}"), "true!");
    assert_eq!(p("{names[0] + \"/\" + names[1]}"), "PLA/PETG");
}

#[test]
fn comparisons() {
    assert_eq!(p("{(0.1 + 0.2 == 0.3)}"), "true");
    assert_eq!(p("{(1 == 1.000000001)}"), "true");
    assert_eq!(p("{(1 == 1.0001)}"), "false");
    assert_eq!(p("{(\"abc\" < \"abd\")}"), "true");
    assert_eq!(p("{(1 == \"1\")}"), "true");
    assert_eq!(p("{(2.5 == \"2.5\")}"), "true");
    assert_eq!(p("{(true == enabled)}"), "true");
    assert_eq!(p("{(true != \"true\")}"), "false");
    assert_eq!(p("{(3 >= 3) and (2 <= 1 or 1 < 2)}"), "true");
    assert!(fails("{(true < false)}").contains("Cannot compare"));
    assert!(fails("{(true == 1)}").contains("Cannot compare"));
    assert!(fails("{(1 and true)}").contains("logical operation"));
}

#[test]
fn regex() {
    assert_eq!(p("{(\"abc\" =~ /b/)}"), "false"); // full match
    assert_eq!(p("{(\"abc\" =~ /.*b.*/)}"), "true");
    assert_eq!(p("{(\"abc\" !~ /.*b.*/)}"), "false");
    assert_eq!(p("{(notes =~ /.*one.*two.*/)}"), "true"); // '.' matches newlines
    assert_eq!(p("{(\"a/b\" =~ /a\\/b/)}"), "true");
    assert_eq!(p("{(names[1] =~ /PE(TG|T)/)}"), "true");
    assert!(fails("{(1 =~ /1/)}").contains("must be a string"));
    assert!(fails("{(\"a\" =~ /(/)}").contains("Regular expression compilation failed"));
    assert_eq!(p("{one_of(names[1], \"PLA\", /PE.*/)}"), "true");
    assert_eq!(p("{one_of(\"x\",)}"), "false");
    // Commas between patterns are optional, the one after the subject is not.
    assert_eq!(p("{one_of(\"b\", \"a\" \"b\")}"), "true");
    assert!(process("{one_of(\"b\" \"b\")}", &mut ctx()).is_err());
    assert!(fails("{one_of(1, \"1\")}").contains("has to be a string"));
}

#[test]
fn functions() {
    assert_eq!(p("{digits(2.5, 0)}"), "3"); // std::round: half away from zero
    assert_eq!(p("{digits(-2.5, 3)}"), " -3");
    assert_eq!(p("{zdigits(-5, 4)}"), "-005");
    assert_eq!(p("{zdigits(0.125, 6, 2)}"), "000.12"); // printf rounds ties to even
    assert_eq!(p("{digits(3, 0, 1)}"), "3.0");
    assert!(fails("{digits(3, 1.5)}").contains("second parameter must be integer"));
    assert!(fails("{digits(3, 1, 1.5)}").contains("third parameter must be integer"));
    assert!(fails("{digits(\"3\", 1)}").contains("Not a numeric type"));
    assert_eq!(p("{int(-2.9)}"), "-2");
    assert_eq!(p("{round(2.5)}"), "3");
    assert!(fails("{int(\"3\")}").contains("Cannot convert to integer"));
    assert!(fails("{round(true)}").contains("Cannot round"));
    assert_eq!(p("{size(temperature)};{empty(empty_ints)}"), "3;true");
    assert!(fails("{size(temperature[0])}").contains("not a vector variable"));
    assert!(fails("{size(count)}").contains("not a vector variable"));
}

#[test]
fn interpolate_table_edges() {
    assert_eq!(
        p("{interpolate_table(175, (0,7000), (150,7000), (200,4000), (270,2000))}"),
        "5500"
    );
    assert_eq!(
        p("{interpolate_table(150, (0,7000), (150,7000), (200,4000))}"),
        "7000"
    );
    assert_eq!(p("{interpolate_table(-0.00005, (0, 1), (10, 2))}"), "1"); // clamped within EPSILON
    assert!(fails("{interpolate_table(11, (0, 1), (10, 2))}").contains("outside the table range"));
    assert!(fails("{interpolate_table(5, (10, 1), (0, 2))}").contains("must be increasing"));
    // A one-point table rejects its own x (PrusaSlicer quirk).
    assert!(fails("{interpolate_table(1, (1, 5))}").contains("outside the table range"));
    assert!(fails("{interpolate_table(\"x\", (1, 5))}").contains("must be a number"));
    // Only single operands are allowed in the table.
    assert!(process("{interpolate_table(1, (0+1, 5))}", &mut ctx()).is_err());
}

#[test]
fn vectors_and_nil() {
    assert_eq!(p("{temperature[1]}"), "220");
    assert_eq!(p("{temperature[idx + 1]}"), "225");
    assert_eq!(p("{temperature[7]}"), "215"); // past the end reads element 0
    assert_eq!(p("{abrasive[1]}"), "true");
    assert_eq!(p("{nozzle_diameter[1]}"), "0.6");
    assert_eq!(
        p("{is_nil(maybe[0])},{is_nil(maybe[1])},{maybe[1]},{is_nil(nil_scalar)}"),
        "true,false,5,true"
    );
    assert!(fails("{maybe[0]}").contains("undefined (nil) element"));
    assert!(fails("{nil_scalar}").contains("undefined (nil) optional variable"));
    assert!(fails("{temperature}").contains("scalar is expected"));
    assert!(fails("{temperature[1.0]}").contains("Non-integer index"));
    assert!(fails("{temperature[-1]}").contains("negative index"));
    assert!(fails("{count[0]}").contains("Cannot index a scalar"));
    assert!(fails("{empty_ints[0]}").contains("empty vector"));
    assert!(fails("{unknown}").contains("Not a variable name"));
}

#[test]
fn legacy_expansion() {
    assert_eq!(p("[temperature]"), "215");
    assert_eq!(p("[temperature_2]"), "225");
    assert_eq!(p("[temperature_]"), "215"); // strtol("") == 0
    assert_eq!(p("[temperature_9]"), "215");
    assert_eq!(p("[temperature[idx]]"), "220");
    assert_eq!(p("[abrasive_1]"), "1");
    assert_eq!(p("[enabled]"), "1");
    assert_eq!(p("[height]"), "0.2");
    assert_eq!(p("[notes]"), "line one\\nline \\\\two"); // scalar strings are C-escaped
    assert_eq!(p("[names_1]"), "PETG");
    let mut c = ctx();
    c.set_current_extruder(1);
    assert_eq!(
        process("[temperature] [nozzle_diameter]", &mut c).unwrap(),
        "220 0.6"
    );
    assert!(fails("[temperature_x]").contains("Invalid vector index"));
    assert!(fails("[count_1]").contains("Trying to index a scalar"));
    assert!(fails("[nope]").contains("Variable does not exist"));
    assert!(fails("[maybe]").contains("nil"));
    assert!(fails("M117 [ok").starts_with("Expecting"));
    // Legacy expansion does not see locals, globals or outputs.
    assert!(fails("{local x = 1}[x]").contains("Variable does not exist"));
}

#[test]
fn text_blocks() {
    assert_eq!(p("a}b"), "a}b");
    assert_eq!(p("{if enabled}yes{else}no{endif}"), "yes");
    assert_eq!(
        p("{if not enabled}yes{elsif count == 3}three{else}no{endif}"),
        "three"
    );
    assert_eq!(p("{if count > 5}big{endif}|"), "|");
    assert_eq!(p("{if enabled}\n  indented\n{endif}"), "\n  indented\n");
    assert_eq!(p("{if enabled then \"x\"; \"y\" else \"z\" endif}"), "xy");
    assert_eq!(p("{ count ; ; height }"), "30.2");
    assert!(fails("{}").contains("Unknown syntax error"));
    assert!(fails("{endif}").contains("Unknown syntax error"));
    assert!(fails("{if 1}x{endif}").contains("Not a boolean expression"));
    assert!(process("{if enabled}x", &mut ctx()).is_err());
}

#[test]
fn prusaslicer_parse_quirks() {
    // A statement starting with "variable =" is an assignment, even for "==".
    assert!(process("{count == 3}", &mut ctx()).is_err());
    assert_eq!(p("{(count == 3)}"), "true");
    assert_eq!(p("{3 == count}"), "true");
    // "<>" never parses.
    assert!(fails("{(1 <> 2)}").contains("Expecting an expression"));
    // A parenthesised first operand on the right of "=" is an initializer list.
    let mut c = ctx();
    c.set_output("out", 0);
    assert!(process("{out = (1 + 2) * 3}", &mut c).is_err());
    process("{out = (1 + 2)}", &mut c).unwrap();
    assert_eq!(c.output("out"), Some(&Var::int(3)));
    // In an inactive branch "local x = y + 1" is a syntax error, "local x = 1 + y" is fine.
    assert!(process("{if false}{local x = count + 1}{endif}", &mut ctx()).is_err());
    assert_eq!(p("{if false}{local x = 1 + count}{endif}ok"), "ok");
    // Unknown names and type errors in inactive branches are fine.
    assert_eq!(
        p("{if false}{nothing + \"x\" * 3}{endif}{false ? 1/0 : 2}"),
        "2"
    );
    // elsif conditions are evaluated even after a branch was taken.
    assert!(fails("{if true}a{elsif 1/0 == 1}b{endif}").contains("Division by zero"));
}

#[test]
fn output_variables() {
    let mut c = ctx();
    c.set_output("position", Var::floats([0.0, 0.0, 0.0]));
    c.set_output("e_retracted", Var::floats([0.0]));
    c.set_output("layer_count", 0);
    c.set_output("label", "");
    c.set_output("flag", false);
    process(
        "{position[2] = position[2] + 5; e_retracted[0] = 1; layer_count = 2.9; label = 3.5; flag = count > 1}",
        &mut c,
    )
    .unwrap();
    assert_eq!(c.output("position"), Some(&Var::floats([0.0, 0.0, 5.0])));
    assert_eq!(c.output("e_retracted"), Some(&Var::floats([1.0])));
    assert_eq!(c.output("layer_count"), Some(&Var::int(2))); // truncated to the variable's type
    assert_eq!(c.output("label"), Some(&Var::string("3.5")));
    assert_eq!(c.output("flag"), Some(&Var::bool(true)));
    process("{position = (1, 2, 3)}", &mut c).unwrap();
    assert_eq!(c.output("position"), Some(&Var::floats([1.0, 2.0, 3.0])));
    process("{position = repeat(3, 7)}", &mut c).unwrap();
    assert_eq!(c.output("position"), Some(&Var::floats([7.0, 7.0, 7.0])));
    process("{position = nozzle_diameter}", &mut c).unwrap();
    assert_eq!(c.output("position"), Some(&Var::floats([0.4, 0.6])));

    let e = process("{flag = 1}", &mut c).unwrap_err();
    assert!(e.message.contains("not a boolean"), "{e}");
    let e = process("{layer_count = \"x\"}", &mut c).unwrap_err();
    assert!(e.message.contains("not a numeric"), "{e}");
    let e = process("{e_retracted[3] = 1}", &mut c).unwrap_err();
    assert!(e.message.contains("Index out of range"), "{e}");
    let e = process("{e_retracted = 1}", &mut c).unwrap_err();
    assert!(e.message.contains("scalar is expected"), "{e}");
    let e = process("{e_retracted = temperature}", &mut c).unwrap_err();
    assert!(e.message.contains("float vector"), "{e}");
    let e = process("{label = (1, 2)}", &mut c).unwrap_err();
    assert!(e.message.contains("vector value to a scalar"), "{e}");
    let e = process("{count = 4}", &mut c).unwrap_err();
    assert!(e.message.contains("read-only"), "{e}");
    let e = process("{local position = 1}", &mut c).unwrap_err();
    assert!(e.message.contains("system output variable"), "{e}");
    let e = process("{local count = 1}", &mut c).unwrap_err();
    assert!(e.message.contains("read-only system dictionary"), "{e}");
    let e = process("{position = repeat(-1, 0)}", &mut c).unwrap_err();
    assert!(e.message.contains("Negative number"), "{e}");
}

#[test]
fn scopes() {
    let mut c = ctx();
    assert_eq!(
        process("{global g = 5}{local l = 1}{g + l}", &mut c).unwrap(),
        "6"
    );
    assert_eq!(c.global("g"), Some(&Var::int(5)));
    assert_eq!(c.get("g"), Some(&Var::int(5)));
    // Globals persist across calls, locals do not.
    assert_eq!(process("{g = g * 2}{g}", &mut c).unwrap(), "10");
    assert!(process("{l}", &mut c).is_err());
    // Redeclaring keeps the existing type.
    assert_eq!(process("{global g = 2.7}{g}", &mut c).unwrap(), "2");
    assert!(
        process("{local g = 1}", &mut c)
            .unwrap_err()
            .message
            .contains("already defined in global scope")
    );
    assert!(
        process("{local l = 1; global l = 2}", &mut c)
            .unwrap_err()
            .message
            .contains("already defined in local scope")
    );
    c.clear_globals();
    assert!(process("{g}", &mut c).is_err());
}

#[test]
fn new_vector_variables() {
    assert_eq!(p("{local v = (1, 2.5)}{v[0]}"), "1");
    assert_eq!(p("{local v = (1, \"a\")}{v[0]}{v[1]}"), "1a");
    assert_eq!(p("{local v = (5)}{size(v)}"), "1"); // parenthesised value makes a vector
    assert_eq!(p("{local v = repeat(2, 0.5)}{v[1]}"), "0.5");
    assert_eq!(p("{local v = maybe}{is_nil(v[0])}"), "true");
    assert_eq!(p("{local v = empty_strings}{size(v)}"), "0");
    assert!(fails("{local v = (1, true)}").contains("Mixing numeric and boolean"));
    assert!(fails("{local v = ()}").contains("empty initializer list"));
    // An empty non-string vector counts as "all nil" (PrusaSlicer quirk).
    assert!(fails("{local v = empty_ints}").contains("undefined (nil)"));
    assert!(fails("{local v = (1, 2); local v = 3}").contains("scalar value to a vector"));
    assert!(fails("{local s = 1; local s = repeat(2, 1)}").contains("vector value to a scalar"));
}

#[test]
fn random_is_injectable() {
    struct Fixed(u64);
    impl RandomSource for Fixed {
        fn next_u64(&mut self) -> u64 {
            self.0
        }
    }
    let mut c = ctx();
    c.set_random_source(Box::new(Fixed(0)));
    assert_eq!(
        process("{random(3, 9)} {random(1.0, 2.0)}", &mut c).unwrap(),
        "3 1"
    );
    c.set_random_source(Box::new(Fixed(u64::MAX)));
    assert_eq!(process("{random(3, 9)}", &mut c).unwrap(), "9");

    let roll = |seed| {
        let mut c = Context::with_seed(seed);
        process(
            "{random(1, 6)},{random(1, 6)},{random(1, 6)},{random(0., 1.)}",
            &mut c,
        )
        .unwrap()
    };
    assert_eq!(roll(42), roll(42));
    let mut c = Context::with_seed(3);
    for _ in 0..200 {
        let v: i64 = process("{random(-2, 2)}", &mut c).unwrap().parse().unwrap();
        assert!((-2..=2).contains(&v));
        let f: f64 = process("{random(0.5, 1.5)}", &mut c)
            .unwrap()
            .parse()
            .unwrap();
        assert!((0.5..1.5).contains(&f));
    }
    assert!(process("{random(5, 1)}", &mut c).is_err());
}

#[test]
fn boolean_mode() {
    let mut c = ctx();
    assert!(!evaluate_boolean("", &mut c).unwrap());
    assert!(evaluate_boolean("count == 3 and names[0] == \"PLA\"", &mut c).unwrap());
    assert!(evaluate_boolean("1 + 1", &mut c).is_err());
    assert!(evaluate_boolean("true false", &mut c).is_err());
}

#[test]
fn errors_have_positions() {
    let e = process("G1 X1\nG1 Y{1 + }\n", &mut ctx()).unwrap_err();
    assert_eq!(e.line, 2);
    assert_eq!(e.column, 10);
    assert_eq!(e.source_line, "G1 Y{1 + }");
    assert_eq!(
        e.to_string(),
        "Parsing error at line 2: Expecting an expression.\nG1 Y{1 + }\n         ^"
    );
    let e = process("{\"a\\q\"}", &mut ctx()).unwrap_err();
    assert_eq!(e.message, "Invalid escape sequence");
    let e = process("{1 é}", &mut ctx()).unwrap_err();
    assert!(e.message.contains("Non-ASCII7"), "{e}");
}

#[test]
fn context_accessors() {
    let mut c = Context::new();
    c.set("a", Var::floats([1.0]));
    c.set(
        "ints_as_floats",
        Var::Vector(ValueType::Float, vec![Value::Int(2)]),
    );
    assert_eq!(c.get("ints_as_floats"), Some(&Var::floats([2.0])));
    assert_eq!(c.remove("a"), Some(Var::floats([1.0])));
    assert_eq!(c.get("a"), None);
    assert_eq!(Var::int(3).as_scalar().and_then(Value::as_f64), Some(3.0));
}
