//! What a character code names.
//!
//! A code is `c<body><variant>`: the first pair picks the race, clan and gender the model is built
//! on, and the second the variant of that body. `human.pbd`'s own tree agrees with the split — every
//! male body derives from a male one, Hrothgar from Roegadyn, and the scales it carries rank
//! Roegadyn tallest and Lalafell smallest.
//!
//! The adult and child variants are the ones the game dresses; the two beside them are older
//! bodies nothing is filed under any more. The child body is one shape shared by every race, which
//! is why the two bodies outside the playable range only appear as children.

/// The bodies a code's first pair names, from `01`.
const BODIES: [(&str, &str); 18] = [
    ("Hyur Midlander", "male"),
    ("Hyur Midlander", "female"),
    ("Hyur Highlander", "male"),
    ("Hyur Highlander", "female"),
    ("Elezen", "male"),
    ("Elezen", "female"),
    ("Miqo'te", "male"),
    ("Miqo'te", "female"),
    ("Roegadyn", "male"),
    ("Roegadyn", "female"),
    ("Lalafell", "male"),
    ("Lalafell", "female"),
    ("Au Ra", "male"),
    ("Au Ra", "female"),
    ("Hrothgar", "male"),
    ("Hrothgar", "female"),
    ("Viera", "male"),
    ("Viera", "female"),
];

/// The two bodies outside the playable range, which carry faces and hair and no body of their own.
/// They follow the same male-then-female pairing as the rest.
const UNPLAYABLE: [(u16, (&str, &str)); 2] = [(91, ("NPC", "male")), (92, ("NPC", "female"))];

/// The variants a code's second pair names. Anything else is shown as its own number.
const VARIANTS: [(u16, &str); 2] = [(1, ""), (4, "child")];

/// What a code stands for, or `None` where it names a body the game does not use.
pub fn name(code: u16) -> Option<String> {
    let (body, variant) = (code / 100, code % 100);
    let (race, gender) = match usize::from(body)
        .checked_sub(1)
        .and_then(|at| BODIES.get(at))
    {
        Some(named) => *named,
        None => UNPLAYABLE.iter().find(|(id, _)| *id == body)?.1,
    };
    Some(match VARIANTS.iter().find(|(id, _)| *id == variant) {
        Some((_, "")) => format!("{race} {gender}"),
        Some((_, kind)) => format!("{race} {gender} ({kind})"),
        None => format!("{race} {gender} ({variant})"),
    })
}

/// The code as it is written, with what it stands for after it.
pub fn described(code: u16) -> String {
    match name(code) {
        Some(name) => format!("c{code:04}  {name}"),
        None => format!("c{code:04}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{described, name};

    #[test]
    fn names_every_body_the_deformers_carry() {
        assert_eq!(name(101).as_deref(), Some("Hyur Midlander male"));
        assert_eq!(name(201).as_deref(), Some("Hyur Midlander female"));
        assert_eq!(name(301).as_deref(), Some("Hyur Highlander male"));
        assert_eq!(name(901).as_deref(), Some("Roegadyn male"));
        assert_eq!(name(1101).as_deref(), Some("Lalafell male"));
        assert_eq!(name(1501).as_deref(), Some("Hrothgar male"));
        assert_eq!(name(1801).as_deref(), Some("Viera female"));
    }

    /// The second pair is the variant, and the file carries three the game does not name.
    #[test]
    fn names_the_variants_apart() {
        assert_eq!(name(104).as_deref(), Some("Hyur Midlander male (child)"));
        assert_eq!(name(102).as_deref(), Some("Hyur Midlander male (2)"));
        assert_eq!(name(9104).as_deref(), Some("NPC male (child)"));
        assert_eq!(name(9204).as_deref(), Some("NPC female (child)"));
    }

    #[test]
    fn a_body_the_game_does_not_use() {
        assert_eq!(name(0), None);
        assert_eq!(name(1901), None);
        assert_eq!(described(1901), "c1901");
    }

    #[test]
    fn writes_the_code_beside_the_name() {
        assert_eq!(described(101), "c0101  Hyur Midlander male");
    }
}
