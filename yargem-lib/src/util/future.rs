use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

/// Which branch of a [`Race`] finished first.
pub enum Either<A, B> {
    Left(A),
    Right(B),
}

/// Drives `left` and `right` together and finishes with whichever completes
/// first. Both stay pinned where the caller put them, so the loser is dropped
/// with the caller's pin, not here.
///
/// `left` is polled first each round, and `right` is not polled at all once
/// `left` is ready: the fixed order keeps behaviour deterministic, and a tie
/// goes to `left` without consuming an event `right` would have taken.
pub struct Race<'a, A: Future, B: Future> {
    left: Pin<&'a mut A>,
    right: Pin<&'a mut B>,
}

impl<'a, A, B> Race<'a, A, B>
where
    A: Future,
    B: Future,
{
    pub fn new(left: Pin<&'a mut A>, right: Pin<&'a mut B>) -> Self {
        Self { left, right }
    }
}

impl<A: Future, B: Future> Future for Race<'_, A, B> {
    type Output = Either<A::Output, B::Output>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        // Both fields are `Pin<&mut _>`, so `Race` is `Unpin`.
        let this = self.as_mut().get_mut();
        if let Poll::Ready(left) = this.left.as_mut().poll(cx) {
            return Poll::Ready(Either::Left(left));
        }
        this.right.as_mut().poll(cx).map(Either::Right)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::Cell,
        future,
        pin::pin,
        task::{Poll, Waker},
    };

    async fn poller(n_polls: u32) {
        let mut polls = 0;
        future::poll_fn(|_| {
            if polls < n_polls {
                polls += 1;
                Poll::Pending
            } else {
                Poll::Ready(())
            }
        })
        .await
    }

    fn race_futures(
        left: Pin<&mut impl Future<Output = ()>>,
        right: Pin<&mut impl Future<Output = ()>>,
        max_polls: u32,
    ) -> Either<(), ()> {
        let mut race = pin!(Race::new(left, right));
        let mut cx = Context::from_waker(Waker::noop());

        let mut loops = 0;
        let poll = loop {
            let poll = race.as_mut().poll(&mut cx);
            if poll.is_ready() || loops >= max_polls {
                break poll;
            }
            loops += 1;
        };
        match poll {
            Poll::Ready(either) => either,
            Poll::Pending => panic!(
                "race unfinished after {} polls, max was {}",
                loops, max_polls
            ),
        }
    }

    #[test]
    fn race_left_wins() {
        let left = pin!(poller(3));
        let right = pin!(poller(10));

        let either = race_futures(left, right, 10);

        assert!(matches!(either, Either::Left(())))
    }

    #[test]
    fn tie_goes_to_left_without_polling_right() {
        let right_polls = Cell::new(0);
        let left = pin!(poller(3));
        // Ready on its 4th poll: the same round as `left`.
        let right = pin!(future::poll_fn(|_| {
            right_polls.set(right_polls.get() + 1);
            if right_polls.get() > 3 {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        }));

        assert!(matches!(race_futures(left, right, 3), Either::Left(())));
        assert_eq!(
            right_polls.get(),
            3,
            "right polled in the round left finished"
        );
    }

    #[test]
    fn race_right_wins() {
        let left = pin!(poller(10));
        let right = pin!(poller(3));

        let either = race_futures(left, right, 10);

        assert!(matches!(either, Either::Right(())))
    }
}
