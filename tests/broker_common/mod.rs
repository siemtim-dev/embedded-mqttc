use rumqttc::{AsyncClient, Event, EventLoop, MqttOptions, Packet, Publish};
use tokio::{sync::mpsc::{channel, Receiver, Sender}, task::JoinHandle};
use tokio_util::sync::CancellationToken;



pub fn create_sinple_client(client_id: &str, config: &super::BrokerConfig) -> (AsyncClient, Receiver<Publish>, CancellationToken) {

    let shutdown_token = CancellationToken::new();

    let (sender, receiver) = channel(32);

    let mut options = MqttOptions::new(client_id, &config.host, config.unwrap_port());

    if let Some(username) = &config.username {
        let password = config.password.as_ref().expect("username is given, password mutst be too");
        options.set_credentials(username, password);
    }

    let (client, event_loop) = AsyncClient::new(options, 16);

    start_event_loop(event_loop, shutdown_token.clone(), sender);

    (client, receiver, shutdown_token)
}

fn start_event_loop(mut event_loop: EventLoop, shutdown_token: CancellationToken, sender: Sender<Publish>) -> JoinHandle<()> {
    tokio::spawn(async move {
        while ! shutdown_token.is_cancelled() {
            tokio::select! {
                event = event_loop.poll() => {
                    match event {
                        Ok(event) => {
                            process_notification(event, &sender).await;
                        },
                        Err(err) => {
                            tracing::error!("error polling event loop: {}", err);
                            break;
                        },
                    }
                },
                _ = shutdown_token.cancelled() => {}
            }
        }
    })
}

async fn process_notification(notification: Event, sender: &Sender<Publish>) {
    if let Event::Incoming(incoming) = notification {
        if let Packet::Publish(publish) = incoming {
            sender.send(publish).await.unwrap();
        }
    }
}

