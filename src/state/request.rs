use core::{cell::RefCell, future::Future, pin::Pin, task::{Context, Poll}};

use embassy_sync::{blocking_mutex::{Mutex, raw::RawMutex}, waitqueue::WakerRegistration};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestNotification {
    NewRequest,
    Disconnect
}

struct RequestStateInner {

    waker: WakerRegistration,
    notification: Option<RequestNotification>

}

impl RequestStateInner {

    fn new() -> Self {
        Self {
            waker: WakerRegistration::new(),
            notification: None
        }
    }

    fn poll_notification(&mut self, cx: &mut Context) -> Poll<RequestNotification>{
        match self.notification {
            Some(RequestNotification::Disconnect) => Poll::Ready(RequestNotification::Disconnect),
            Some(other) => {
                self.notification = None; // Consume last notification
                Poll::Ready(other)
            },
            None => {
                self.waker.register(cx.waker());
                Poll::Pending
            }
        }
    }

    fn notify(&mut self, notification: RequestNotification) {
        self.notification = match self.notification.take() {
            Some(RequestNotification::Disconnect) => Some(RequestNotification::Disconnect), // Never overwrite a disconnect
            _ => Some(notification),
        };
    }

}

pub struct NotificationFuture<'a, M: RawMutex> {
    state: &'a RequestState<M>
}

impl<'a, M: RawMutex> Future for NotificationFuture<'a, M> {
    type Output = RequestNotification;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.state.inner.lock(|inner | {
            let mut inner = inner.borrow_mut();
            inner.poll_notification(cx)
        })
    }
}

pub struct RequestState<M: RawMutex>{
    inner: Mutex<M, RefCell<RequestStateInner>>
}

impl<M: RawMutex> RequestState<M> {

    pub fn new() -> Self {
        Self {
            inner: Mutex::new(RefCell::new(RequestStateInner::new()))
        }
    }

    pub fn next_notification(&self) -> NotificationFuture<'_, M> {
        NotificationFuture{
            state: self
        }
    }

    pub fn notify(&self, notification: RequestNotification) {
        self.inner.lock(|inner| inner.borrow_mut().notify(notification))
    }

    pub fn notify_new_request(&self) {
        self.notify(RequestNotification::NewRequest);
    }

    pub fn notify_disconnect(&self) {
        self.notify(RequestNotification::Disconnect);
    }

}

#[cfg(test)]
mod tests {
    use core::pin::Pin;

    use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

    use crate::state::request::{RequestNotification, RequestState};

    use crate::testutils::*;

    #[test]
    fn test_notify() {
        let request_state = RequestState::<CriticalSectionRawMutex>::new();

        let mut notification_fut = request_state.next_notification();
        let mut notification_fut = Pin::new(&mut notification_fut);

        assert_pending(notification_fut.as_mut());

        request_state.notify(RequestNotification::NewRequest);
        let value = assert_ready(notification_fut);
        assert_eq!(value, RequestNotification::NewRequest);

        // again
        let mut notification_fut = request_state.next_notification();
        let notification_fut = Pin::new(&mut notification_fut);
        assert_pending(notification_fut);
    }

    #[test]
    fn test_multi_notify() {
        let request_state = RequestState::<CriticalSectionRawMutex>::new();

        let mut notification_fut = request_state.next_notification();
        let mut notification_fut = Pin::new(&mut notification_fut);

        assert_pending(notification_fut.as_mut());

        request_state.notify(RequestNotification::NewRequest);
        request_state.notify(RequestNotification::NewRequest);
        request_state.notify(RequestNotification::NewRequest);
        request_state.notify(RequestNotification::NewRequest);

        let value = assert_ready(notification_fut);
        assert_eq!(value, RequestNotification::NewRequest);

        // again
        let mut notification_fut = request_state.next_notification();
        let notification_fut = Pin::new(&mut notification_fut);
        assert_pending(notification_fut);
    }

    #[test]
    fn test_disconnect_notify() {
        let request_state = RequestState::<CriticalSectionRawMutex>::new();

        let mut notification_fut = request_state.next_notification();
        let mut notification_fut = Pin::new(&mut notification_fut);

        assert_pending(notification_fut.as_mut());

        request_state.notify(RequestNotification::Disconnect);

        let value = assert_ready(notification_fut);
        assert_eq!(value, RequestNotification::Disconnect);

        // again
        let value = assert_ready_pin(request_state.next_notification());
        assert_eq!(value, RequestNotification::Disconnect);
    }

    #[test]
    fn test_disconnect_multi_notify() {
        let request_state = RequestState::<CriticalSectionRawMutex>::new();

        let mut notification_fut = request_state.next_notification();
        let mut notification_fut = Pin::new(&mut notification_fut);

        assert_pending(notification_fut.as_mut());

        request_state.notify(RequestNotification::Disconnect);
        request_state.notify(RequestNotification::NewRequest);
        request_state.notify(RequestNotification::NewRequest);

        let value = assert_ready(notification_fut);
        assert_eq!(value, RequestNotification::Disconnect);

        // again
        let value = assert_ready_pin(request_state.next_notification());
        assert_eq!(value, RequestNotification::Disconnect);
    }

}