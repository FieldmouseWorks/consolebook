//! Closed vocabularies stored as text: one list per vocabulary, from
//! which the enum, its stored names, its parser, and its `ALL` are all
//! generated, so a value cannot exist without a stored name, a parse
//! arm, and a place in `ALL` (docs/formats/installation-export.md,
//! Vocabularies; CONTRIBUTING.md, correct the class).

/// Declares a closed vocabulary:
///
/// ```ignore
/// closed_vocabulary! {
///     /// Docs.
///     pub enum Kind {
///         First => "first",
///         Second => "second",
///     }
/// }
/// ```
///
/// generates `enum Kind` (`Debug, Clone, Copy, PartialEq, Eq`),
/// `Kind::ALL` (every value once, in declaration order),
/// `Kind::as_str`, and `Kind::parse` (the stored name back to its value,
/// or `None` for a name this build does not know). Values are only ever
/// added: an older installation's stored rows must keep reading.
macro_rules! closed_vocabulary {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $( $(#[$variant_meta:meta])* $variant:ident => $stored:literal ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        $vis enum $name {
            $( $(#[$variant_meta])* $variant ),+
        }

        impl $name {
            /// Every value, once, in declaration order.
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            /// The stored name.
            #[must_use]
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $stored),+
                }
            }

            /// The value a stored name names, or `None` for a name this
            /// build does not know.
            #[must_use]
            pub fn parse(stored: &str) -> Option<Self> {
                match stored {
                    $($stored => Some(Self::$variant),)+
                    _ => None,
                }
            }
        }
    };
}

pub(crate) use closed_vocabulary;
