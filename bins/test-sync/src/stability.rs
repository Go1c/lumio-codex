#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stability {
    Collecting { identical: usize },
    Rejected,
    Stable,
}

pub fn classify_stability<T: Eq>(samples: &[T], acceptable: impl Fn(&T) -> bool) -> Stability {
    let Some(last) = samples.last() else {
        return Stability::Collecting { identical: 0 };
    };
    let identical = samples
        .iter()
        .rev()
        .take_while(|sample| *sample == last)
        .count();
    if identical < 3 {
        return Stability::Collecting { identical };
    }
    if acceptable(last) {
        Stability::Stable
    } else {
        Stability::Rejected
    }
}
