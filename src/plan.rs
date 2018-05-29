//! The `plan` module provides a domain-specific language for payment plans. Users create Budget objects that
//! are given to an interpreter. The interpreter listens for `Witness` transactions,
//! which it uses to reduce the payment plan. When the plan is reduced to a
//! `Payment`, the payment is executed.

use chrono::prelude::*;
use signature::PublicKey;
use std::mem;

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub enum Witness {
    Timestamp(DateTime<Utc>),
    Signature(PublicKey),
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub enum Condition {
    Timestamp(DateTime<Utc>),
    Signature(PublicKey),
}

impl Condition {
    /// Return true if the given Witness satisfies this Condition.
    pub fn is_satisfied(&self, witness: &Witness) -> bool {
        match (self, witness) {
            (&Condition::Signature(ref pubkey), &Witness::Signature(ref from)) => pubkey == from,
            (&Condition::Timestamp(ref dt), &Witness::Timestamp(ref last_time)) => dt <= last_time,
            _ => false,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct Payment {
    pub tokens: i64,
    pub to: PublicKey,
}

pub trait PaymentPlan {
    /// Return Payment if the spending plan requires no additional Witnesses.
    fn final_payment(&self) -> Option<Payment>;

    /// Return true if the plan spends exactly `spendable_tokens`.
    fn verify(&self, spendable_tokens: i64) -> bool;

    /// Apply a witness to the spending plan to see if the plan can be reduced.
    /// If so, modify the plan in-place.
    fn apply_witness(&mut self, witness: &Witness);
}

#[repr(C)]
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub enum Budget {
    Pay(Payment),
    After(Condition, Payment),
    Race((Condition, Payment), (Condition, Payment)),
}

impl Budget {
    /// Create the simplest spending plan - one that pays `tokens` to PublicKey.
    pub fn new_payment(tokens: i64, to: PublicKey) -> Self {
        Budget::Pay(Payment { tokens, to })
    }

    /// Create a spending plan that pays `tokens` to `to` after being witnessed by `from`.
    pub fn new_authorized_payment(from: PublicKey, tokens: i64, to: PublicKey) -> Self {
        Budget::After(Condition::Signature(from), Payment { tokens, to })
    }

    /// Create a spending plan that pays `tokens` to `to` after the given DateTime.
    pub fn new_future_payment(dt: DateTime<Utc>, tokens: i64, to: PublicKey) -> Self {
        Budget::After(Condition::Timestamp(dt), Payment { tokens, to })
    }

    /// Create a spending plan that pays `tokens` to `to` after the given DateTime
    /// unless cancelled by `from`.
    pub fn new_cancelable_future_payment(
        dt: DateTime<Utc>,
        from: PublicKey,
        tokens: i64,
        to: PublicKey,
    ) -> Self {
        Budget::Race(
            (Condition::Timestamp(dt), Payment { tokens, to }),
            (Condition::Signature(from), Payment { tokens, to: from }),
        )
    }
}

impl PaymentPlan for Budget {
    /// Return Payment if the spending plan requires no additional Witnesses.
    fn final_payment(&self) -> Option<Payment> {
        match *self {
            Budget::Pay(ref payment) => Some(payment.clone()),
            _ => None,
        }
    }

    /// Return true if the plan spends exactly `spendable_tokens`.
    fn verify(&self, spendable_tokens: i64) -> bool {
        match *self {
            Budget::Pay(ref payment) | Budget::After(_, ref payment) => {
                payment.tokens == spendable_tokens
            }
            Budget::Race(ref a, ref b) => {
                a.1.tokens == spendable_tokens && b.1.tokens == spendable_tokens
            }
        }
    }

    /// Apply a witness to the spending plan to see if the plan can be reduced.
    /// If so, modify the plan in-place.
    fn apply_witness(&mut self, witness: &Witness) {
        let new_payment = match *self {
            Budget::After(ref cond, ref payment) if cond.is_satisfied(witness) => Some(payment),
            Budget::Race((ref cond, ref payment), _) if cond.is_satisfied(witness) => Some(payment),
            Budget::Race(_, (ref cond, ref payment)) if cond.is_satisfied(witness) => Some(payment),
            _ => None,
        }.cloned();

        if let Some(payment) = new_payment {
            mem::replace(self, Budget::Pay(payment));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signature_satisfied() {
        let sig = PublicKey::default();
        assert!(Condition::Signature(sig).is_satisfied(&Witness::Signature(sig)));
    }

    #[test]
    fn test_timestamp_satisfied() {
        let dt1 = Utc.ymd(2014, 11, 14).and_hms(8, 9, 10);
        let dt2 = Utc.ymd(2014, 11, 14).and_hms(10, 9, 8);
        assert!(Condition::Timestamp(dt1).is_satisfied(&Witness::Timestamp(dt1)));
        assert!(Condition::Timestamp(dt1).is_satisfied(&Witness::Timestamp(dt2)));
        assert!(!Condition::Timestamp(dt2).is_satisfied(&Witness::Timestamp(dt1)));
    }

    #[test]
    fn test_verify_plan() {
        let dt = Utc.ymd(2014, 11, 14).and_hms(8, 9, 10);
        let from = PublicKey::default();
        let to = PublicKey::default();
        assert!(Budget::new_payment(42, to).verify(42));
        assert!(Budget::new_authorized_payment(from, 42, to).verify(42));
        assert!(Budget::new_future_payment(dt, 42, to).verify(42));
        assert!(Budget::new_cancelable_future_payment(dt, from, 42, to).verify(42));
    }

    #[test]
    fn test_authorized_payment() {
        let from = PublicKey::default();
        let to = PublicKey::default();

        let mut plan = Budget::new_authorized_payment(from, 42, to);
        plan.apply_witness(&Witness::Signature(from));
        assert_eq!(plan, Budget::new_payment(42, to));
    }

    #[test]
    fn test_future_payment() {
        let dt = Utc.ymd(2014, 11, 14).and_hms(8, 9, 10);
        let to = PublicKey::default();

        let mut plan = Budget::new_future_payment(dt, 42, to);
        plan.apply_witness(&Witness::Timestamp(dt));
        assert_eq!(plan, Budget::new_payment(42, to));
    }

    #[test]
    fn test_cancelable_future_payment() {
        let dt = Utc.ymd(2014, 11, 14).and_hms(8, 9, 10);
        let from = PublicKey::default();
        let to = PublicKey::default();

        let mut plan = Budget::new_cancelable_future_payment(dt, from, 42, to);
        plan.apply_witness(&Witness::Timestamp(dt));
        assert_eq!(plan, Budget::new_payment(42, to));

        let mut plan = Budget::new_cancelable_future_payment(dt, from, 42, to);
        plan.apply_witness(&Witness::Signature(from));
        assert_eq!(plan, Budget::new_payment(42, from));
    }
}
