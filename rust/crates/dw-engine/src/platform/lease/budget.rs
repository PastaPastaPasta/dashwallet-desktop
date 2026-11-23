//! A lease purpose's budget with authority generations (E0-04 §4.2, §4.3).
//!
//! Generation 0's ceiling is the original grant's cap; each rebind opens
//! the next with `ceiling = min(available now, fresh cap)`. A charge is
//! tagged with its generation and a refund restores only that generation's
//! accounting, so a refund never lifts a later ceiling (I13). Everything is
//! checked arithmetic: an overflow refuses the charge.

/// One generation: its ceiling and the charges made under it that were not
/// refunded. `charged <= ceiling` always holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Generation {
    ceiling: u64,
    charged: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Budget {
    gens: Vec<Generation>,
}

/// A charge made under a generation; the only way to refund it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Charge {
    generation: usize,
    pub(crate) amount: u64,
}

/// A charge the current generation cannot cover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Exceeded {
    pub(crate) needed: u64,
    pub(crate) remaining: u64,
}

impl Budget {
    pub(crate) fn new(ceiling: u64) -> Self {
        Self {
            gens: vec![Generation {
                ceiling,
                charged: 0,
            }],
        }
    }

    fn current(&self) -> &Generation {
        self.gens.last().expect("a budget has a generation")
    }

    /// Whether the original grant carried this purpose.
    pub(crate) fn granted(&self) -> bool {
        self.gens[0].ceiling > 0
    }

    pub(crate) fn ceiling(&self) -> u64 {
        self.current().ceiling
    }

    /// The unrefunded charges of the current generation.
    pub(crate) fn spent(&self) -> u64 {
        self.current().charged
    }

    /// What the current generation can still cover.
    pub(crate) fn available(&self) -> u64 {
        let g = self.current();
        g.ceiling - g.charged
    }

    pub(crate) fn charge(&mut self, amount: u64) -> Result<Charge, Exceeded> {
        let generation = self.gens.len() - 1;
        let g = &mut self.gens[generation];
        match g.charged.checked_add(amount) {
            Some(total) if total <= g.ceiling => {
                g.charged = total;
                Ok(Charge { generation, amount })
            }
            _ => Err(Exceeded {
                needed: amount,
                remaining: g.ceiling - g.charged,
            }),
        }
    }

    /// Restores `charge` to its own generation only.
    pub(crate) fn refund(&mut self, charge: Charge) {
        if let Some(g) = self.gens.get_mut(charge.generation) {
            g.charged = g.charged.saturating_sub(charge.amount);
        }
    }

    /// Opens the next generation under fresh authority capped at `fresh`.
    pub(crate) fn rebind(&mut self, fresh: u64) {
        let ceiling = self.available().min(fresh);
        self.gens.push(Generation {
            ceiling,
            charged: 0,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn charges_are_checked_against_the_current_generation() {
        let mut b = Budget::new(100);
        let c = b.charge(60).unwrap();
        assert_eq!(
            b.charge(41),
            Err(Exceeded {
                needed: 41,
                remaining: 40
            })
        );
        assert_eq!(
            b.charge(u64::MAX).unwrap_err().remaining,
            40,
            "overflow refuses"
        );
        b.refund(c);
        assert_eq!(b.available(), 100);
    }

    #[test]
    fn a_rebind_takes_the_minimum_and_refunds_never_lift_it() {
        let mut b = Budget::new(100);
        let old = b.charge(70).unwrap();
        b.rebind(1_000);
        assert_eq!(b.ceiling(), 30, "min(available 30, fresh 1000)");
        b.refund(old);
        assert_eq!(b.available(), 30, "the refund restored generation 0 only");
        let c1 = b.charge(20).unwrap();
        b.rebind(50);
        assert_eq!(b.ceiling(), 10, "min(available 10, fresh 50)");
        b.refund(c1);
        assert_eq!(b.ceiling(), 10, "a second rebind is not lifted either");
        b.rebind(0);
        assert_eq!(b.available(), 0, "a purpose the fresh grants lack gets 0");
        assert!(b.charge(1).is_err());
        assert!(b.charge(0).is_ok());
    }
}
