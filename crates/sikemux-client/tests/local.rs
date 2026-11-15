use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Duration;

use sikemux_client::local::{self, Listener, Split};
use sikemux_wire::protocol::{encode_frame, read_frame, read_frame_sync, FrameKind};
use tokio::io::AsyncWriteExt;

fn unused_path(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("sikemux-{name}-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&path);
    path
}

fn frame(text: &str) -> Vec<u8> {
    encode_frame(FrameKind::Control, &[text.as_bytes()])
}

#[tokio::test(flavor = "multi_thread")]
async fn a_frame_each_way_with_the_peer_known() {
    let path = unused_path("local");
    let mut listener = Listener::bind(&path).unwrap();
    let core = tokio::spawn(async move {
        for _ in 0..2 {
            let connection = listener.accept().await.unwrap();
            let peer = local::peer(&connection).expect("the peer of a connection");
            assert_eq!(peer.pid(), std::process::id());
            #[cfg(not(target_vendor = "apple"))]
            assert_eq!(
                peer.program().unwrap().canonicalize().unwrap(),
                std::env::current_exe().unwrap().canonicalize().unwrap()
            );
            let (mut read, mut write) = connection.split();
            let asked = read_frame(&mut read).await.unwrap().unwrap();
            assert_eq!(asked.payload, b"ping");
            write.write_all(&frame("pong")).await.unwrap();
        }
    });

    let (mut read, mut write) = local::connect(&path).await.unwrap().split();
    write.write_all(&frame("ping")).await.unwrap();
    assert_eq!(
        read_frame(&mut read).await.unwrap().unwrap().payload,
        b"pong"
    );

    let blocking = path.clone();
    std::thread::spawn(move || {
        let mut stream = local::connect_sync(&blocking, Duration::from_secs(5)).unwrap();
        stream.write_all(&frame("ping")).unwrap();
        assert_eq!(
            read_frame_sync(&mut stream).unwrap().unwrap().payload,
            b"pong"
        );
    })
    .join()
    .unwrap();

    core.await.unwrap();
    let _ = std::fs::remove_file(&path);
}

#[test]
fn nothing_listening_is_not_found() {
    let path = unused_path("nobody");
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let error = runtime.block_on(local::connect(&path)).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::NotFound);
    let Err(error) = local::connect_sync(&path, Duration::from_secs(1)) else {
        panic!("connected with nothing listening");
    };
    assert_eq!(error.kind(), io::ErrorKind::NotFound);
}
