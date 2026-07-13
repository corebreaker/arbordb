//! Extra `#[derive(AData)]` coverage: the `rename_all` casings (on both struct
//! fields and enum variants) and attribute combinations not exercised by `derive.rs`.

#![cfg(feature = "derive")]
// Some coverage structs use deliberately degenerate field names (`a__b`, `__`) to drive
// the casing helpers' empty-segment path; the derived accessors inherit those names.
#![allow(non_snake_case)]

use arbordb::{AData, ArborDb};

/// Stores then loads `value`, asserting it survives — driving the derived `store` and
/// `load`, which embed whatever field/variant names the attributes produced.
fn roundtrip<T>(value: T)
where
    T: AData + PartialEq + std::fmt::Debug, {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    {
        let w = table.write().unwrap();
        w.store::<T>("x", &value).unwrap();
        w.commit().unwrap();
    }

    let r = table.read().unwrap();
    assert_eq!(r.load::<T>("x").unwrap(), Some(value));
}

// ---- rename_all on struct fields: exercises `RenameRule::apply_to_field` ----

#[derive(AData, Debug, PartialEq)]
#[arbor(rename_all = "lowercase")]
struct LowerFields {
    full_name: i64,
}

#[derive(AData, Debug, PartialEq)]
#[arbor(rename_all = "UPPERCASE")]
struct UpperFields {
    full_name: i64,
}

#[derive(AData, Debug, PartialEq)]
#[arbor(rename_all = "PascalCase")]
struct PascalFields {
    full_name: i64,
}

#[derive(AData, Debug, PartialEq)]
#[arbor(rename_all = "SCREAMING_SNAKE_CASE")]
struct ScreamingSnakeFields {
    full_name: i64,
}

#[derive(AData, Debug, PartialEq)]
#[arbor(rename_all = "kebab-case")]
struct KebabFields {
    full_name: i64,
}

#[derive(AData, Debug, PartialEq)]
#[arbor(rename_all = "SCREAMING-KEBAB-CASE")]
struct ScreamingKebabFields {
    full_name: i64,
}

#[test]
fn rename_all_struct_casings_round_trip() {
    roundtrip(LowerFields {
        full_name: 1
    });
    roundtrip(UpperFields {
        full_name: 2
    });
    roundtrip(PascalFields {
        full_name: 3
    });
    roundtrip(ScreamingSnakeFields {
        full_name: 4
    });
    roundtrip(KebabFields {
        full_name: 5
    });
    roundtrip(ScreamingKebabFields {
        full_name: 6
    });
}

// ---- rename_all on enum variants: exercises `RenameRule::apply_to_variant` ----

#[derive(AData, Debug, PartialEq)]
#[arbor(rename_all = "lowercase")]
enum LowerVariants {
    TrafficLight,
    Payload(i64),
}

#[derive(AData, Debug, PartialEq)]
#[arbor(rename_all = "UPPERCASE")]
enum UpperVariants {
    TrafficLight,
    Payload(i64),
}

#[derive(AData, Debug, PartialEq)]
#[arbor(rename_all = "camelCase")]
enum CamelVariants {
    TrafficLight,
    Payload(i64),
}

#[derive(AData, Debug, PartialEq)]
#[arbor(rename_all = "snake_case")]
enum SnakeVariants {
    TrafficLight,
    Payload(i64),
}

#[derive(AData, Debug, PartialEq)]
#[arbor(rename_all = "SCREAMING_SNAKE_CASE")]
enum ScreamingSnakeVariants {
    TrafficLight,
    Payload(i64),
}

#[derive(AData, Debug, PartialEq)]
#[arbor(rename_all = "kebab-case")]
enum KebabVariants {
    TrafficLight,
    Payload(i64),
}

#[derive(AData, Debug, PartialEq)]
#[arbor(rename_all = "SCREAMING-KEBAB-CASE")]
enum ScreamingKebabVariants {
    TrafficLight,
    Payload(i64),
}

// ---- untagged enum with a multi-element tuple and a struct variant ----
// Exercises the untagged-load codegen for those two shapes (each tried in order).

#[derive(AData, Debug, PartialEq)]
#[arbor(untagged)]
enum UntaggedShapes {
    Pair(i64, i64),
    Named { a: i64, b: String },
}

#[test]
fn untagged_multi_element_and_struct_variants_round_trip() {
    roundtrip(UntaggedShapes::Pair(1, 2));
    roundtrip(UntaggedShapes::Named {
        a: 3,
        b: String::from("x"),
    });
}

// ---- reachable attribute-parsing paths not otherwise exercised ----

/// A `skip_store` field: not written, and restored from its `default` on load.
#[derive(AData, Debug, PartialEq)]
struct SkipStore {
    kept:      i64,
    #[arbor(skip_store, default)]
    ephemeral: i64,
}

#[test]
fn skip_store_field_falls_back_to_its_default() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    {
        let w = table.write().unwrap();
        w.store::<SkipStore>(
            "x",
            &SkipStore {
                kept:      7,
                ephemeral: 99,
            },
        )
        .unwrap();
        w.commit().unwrap();
    }

    let r = table.read().unwrap();
    assert_eq!(
        r.load::<SkipStore>("x").unwrap(),
        Some(SkipStore {
            kept:      7,
            ephemeral: 0,
        })
    );
}

/// A non-`arbor` attribute on a field and on a variant is skipped by the parser.
#[derive(AData, Debug, PartialEq)]
struct DocumentedField {
    /// A doc comment is a non-`arbor` attribute the field parser walks past.
    value: i64,
}

#[derive(AData, Debug, PartialEq)]
enum DocumentedVariant {
    /// A doc comment is a non-`arbor` attribute the variant parser walks past.
    Unit,
    Payload(i64),
}

#[test]
fn non_arbor_attributes_are_ignored() {
    roundtrip(DocumentedField {
        value: 3
    });
    roundtrip(DocumentedVariant::Unit);
    roundtrip(DocumentedVariant::Payload(4));
}

/// Field names with empty `_`-segments drive the casing helpers' empty-segment path.
#[derive(AData, Debug, PartialEq)]
#[arbor(rename_all = "PascalCase")]
#[allow(non_snake_case)]
struct PascalUnderscoreEdges {
    a__b: i64,
}

#[derive(AData, Debug, PartialEq)]
#[arbor(rename_all = "camelCase")]
#[allow(non_snake_case)]
struct CamelUnderscoreEdges {
    __: i64,
}

#[test]
fn rename_all_handles_empty_underscore_segments() {
    roundtrip(PascalUnderscoreEdges {
        a__b: 1
    });
    roundtrip(CamelUnderscoreEdges {
        __: 2
    });
}

#[test]
fn rename_all_variant_casings_round_trip() {
    roundtrip(LowerVariants::TrafficLight);
    roundtrip(LowerVariants::Payload(1));
    roundtrip(UpperVariants::TrafficLight);
    roundtrip(UpperVariants::Payload(2));
    roundtrip(CamelVariants::TrafficLight);
    roundtrip(CamelVariants::Payload(3));
    roundtrip(SnakeVariants::TrafficLight);
    roundtrip(SnakeVariants::Payload(4));
    roundtrip(ScreamingSnakeVariants::TrafficLight);
    roundtrip(ScreamingSnakeVariants::Payload(5));
    roundtrip(KebabVariants::TrafficLight);
    roundtrip(KebabVariants::Payload(6));
    roundtrip(ScreamingKebabVariants::TrafficLight);
    roundtrip(ScreamingKebabVariants::Payload(7));
}
