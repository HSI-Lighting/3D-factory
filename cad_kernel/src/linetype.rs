// Linetype table — a registry of named dash/gap patterns.
//
// Pattern semantics: a slice of alternating dash-then-gap lengths in world
// units. Positive = pen down (dash); negative would be a pen-down with a
// shape (not modelled yet). Empty slice = continuous.
//
// LinetypeId is a stable index. Built-in id 0 = "Continuous" — always
// present so anything that lacks an explicit linetype renders solid.

#[derive(Clone, Debug)]
pub struct Linetype {
    pub name:        String,
    pub description: String,
    /// Dash/gap pattern in world units. Even index = dash length,
    /// odd index = gap length. Empty = continuous.
    pub pattern:     Vec<f32>,
}

impl Linetype {
    pub fn continuous() -> Self {
        Self {
            name:        "Continuous".into(),
            description: "Solid line".into(),
            pattern:     Vec::new(),
        }
    }

    /// Named linetype from an explicit dash/gap pattern (all POSITIVE world
    /// units; even index = dash, odd index = gap; a very short dash renders
    /// as a dot). General constructor for the standard set.
    pub fn new(name: &str, pattern: &[f32]) -> Self {
        Self { name: name.into(), description: String::new(), pattern: pattern.to_vec() }
    }

    /// Simple dashed pattern: dash_len, gap_len.
    pub fn dashed(name: &str, dash: f32, gap: f32) -> Self {
        Self {
            name:        name.into(),
            description: format!("__ __ __  ({} / {})", dash, gap),
            pattern:     vec![dash, gap],
        }
    }

    /// Dash-dot pattern.
    pub fn dash_dot(name: &str, dash: f32, gap: f32) -> Self {
        Self {
            name:        name.into(),
            description: format!("__ . __ . __  ({} / {})", dash, gap),
            pattern:     vec![dash, gap, 0.0, gap],
        }
    }

    pub fn is_continuous(&self) -> bool { self.pattern.is_empty() }
}

#[derive(Clone)]
pub struct LinetypeTable {
    pub linetypes: Vec<Linetype>,
}

impl LinetypeTable {
    /// The standard linetype set (LibreCAD's), in the same order as its
    /// picker: Continuous (id 0), then six families — Dot / Dash / Dash Dot
    /// / Divide / Center / Border — each in normal, tiny, small (·2) and
    /// large (·X2) sizes. Patterns are LibreCAD's, converted to our
    /// all-positive [dash, gap, …] convention (a 0.15–0.2 dash = a dot).
    pub fn with_defaults() -> Self {
        let lt = Linetype::new;
        Self {
            linetypes: vec![
                Linetype::continuous(),                                  // 0
                // ---- Dot ----
                lt("Dot",            &[0.2, 6.2]),                       // 1
                lt("Dot (tiny)",     &[0.15, 1.0]),                      // 2
                lt("Dot (small)",    &[0.2, 3.1]),                       // 3
                lt("Dot (large)",    &[0.2, 12.4]),                      // 4
                // ---- Dash ----
                lt("Dash",           &[12.0, 6.0]),                      // 5
                lt("Dash (tiny)",    &[2.0, 1.0]),                       // 6
                lt("Dash (small)",   &[6.0, 3.0]),                       // 7
                lt("Dash (large)",   &[24.0, 12.0]),                     // 8
                // ---- Dash Dot ----
                lt("Dash Dot",         &[12.0, 5.0, 0.2, 5.0]),          // 9
                lt("Dash Dot (tiny)",  &[2.0, 2.0, 0.15, 2.0]),         // 10
                lt("Dash Dot (small)", &[6.0, 2.5, 0.2, 2.5]),         // 11
                lt("Dash Dot (large)", &[24.0, 8.0, 0.2, 8.0]),        // 12
                // ---- Divide ----
                lt("Divide",         &[12.0, 4.9, 0.2, 4.9, 0.2, 4.9]),  // 13
                lt("Divide (tiny)",  &[2.0, 0.7, 0.15, 0.7, 0.15, 0.7]), // 14
                lt("Divide (small)", &[6.0, 1.9, 0.2, 1.9, 0.2, 1.9]),   // 15
                lt("Divide (large)", &[24.0, 8.0, 0.2, 8.0, 0.2, 8.0]),  // 16
                // ---- Center ----
                lt("Center",         &[32.0, 6.0, 6.0, 6.0]),            // 17
                lt("Center (tiny)",  &[5.0, 1.0, 1.0, 1.0]),            // 18
                lt("Center (small)", &[16.0, 3.0, 3.0, 3.0]),           // 19
                lt("Center (large)", &[64.0, 12.0, 12.0, 12.0]),        // 20
                // ---- Border ----
                lt("Border",         &[12.0, 4.0, 12.0, 4.0, 0.2, 4.0]),  // 21
                lt("Border (tiny)",  &[2.0, 1.0, 2.0, 1.0, 0.15, 1.0]),   // 22
                lt("Border (small)", &[6.0, 3.0, 6.0, 3.0, 0.2, 3.0]),    // 23
                lt("Border (large)", &[24.0, 8.0, 24.0, 8.0, 0.2, 8.0]),  // 24
            ],
        }
    }

    /// The reserved id of the "Continuous" linetype.
    pub const CONTINUOUS: u32 = 0;

    pub fn get(&self, id: u32) -> Option<&Linetype> {
        self.linetypes.get(id as usize)
    }

    pub fn add(&mut self, lt: Linetype) -> u32 {
        let id = self.linetypes.len() as u32;
        self.linetypes.push(lt);
        id
    }

    pub fn find(&self, name: &str) -> Option<u32> {
        self.linetypes.iter().position(|l| l.name.eq_ignore_ascii_case(name))
            .map(|i| i as u32)
    }

    pub fn len(&self) -> usize { self.linetypes.len() }
    pub fn is_empty(&self) -> bool { self.linetypes.is_empty() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_have_continuous_at_id_zero() {
        let t = LinetypeTable::with_defaults();
        assert!(t.get(LinetypeTable::CONTINUOUS).unwrap().is_continuous());
    }

    #[test]
    fn find_is_case_insensitive() {
        let t = LinetypeTable::with_defaults();
        assert_eq!(t.find("continuous"), Some(0));
        assert_eq!(t.find("DASH"), Some(5));
        assert_eq!(t.find("nope"), None);
    }

    #[test]
    fn full_standard_set_present() {
        let t = LinetypeTable::with_defaults();
        // Continuous + 6 families × 4 sizes = 25.
        assert_eq!(t.len(), 25);
        for n in ["Dot", "Dot (tiny)", "Dash", "Dash Dot", "Divide",
                  "Center", "Border (large)"] {
            assert!(t.find(n).is_some(), "missing linetype '{}'", n);
        }
        // The Dot family encodes a dot as a tiny dash + long gap.
        let dot = t.get(t.find("Dot").unwrap()).unwrap();
        assert_eq!(dot.pattern.len(), 2);
        assert!(dot.pattern[0] < 1.0 && dot.pattern[1] > 1.0);
    }
}
