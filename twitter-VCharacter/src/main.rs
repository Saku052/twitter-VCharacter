//! 投稿ボットの入口。引数でサブコマンドを切り替える。
//!
//! Railpack は複数バイナリを扱えない（bin ディレクトリへのコピーが衝突する）ため、
//! planner / tick は別バイナリにせず、同じ実行ファイルのサブコマンドとして持つ。
//! Railway 側は startCommand で使い分ける。
//!
//!   （引数なし）  … メモを1件投稿する（旧来の固定 cron 用）
//!   plan         … 週次の投稿予定を抽選する
//!   tick         … 期限の来た予定を1件実行する

use std::time::Duration;
use tokio::time::timeout;

use vcharacter::cli;
use vcharacter::publish::publish_once;

/// tick 1回の上限。cron 間隔（5分）より短くして、次の起動を止めない
const TICK_TIMEOUT: Duration = Duration::from_secs(230);

#[tokio::main]
async fn main() {
    let cmd = std::env::args().nth(1).unwrap_or_default();

    let result = match cmd.as_str() {
        "plan" => cli::planner::run().await,
        "tick" => match timeout(TICK_TIMEOUT, cli::tick::run()).await {
            Ok(r) => r,
            // ここで落としておかないと、次以降の cron が延々とスキップされる
            Err(_) => Err(anyhow::anyhow!(
                "{}秒で完了しなかったため打ち切りました",
                TICK_TIMEOUT.as_secs()
            )),
        },
        "" => publish_once().await.map(|_| ()),
        other => Err(anyhow::anyhow!("不明なサブコマンド: {}", other)),
    };

    // 失敗を Railway 上で FAILED として見えるようにする
    // （終了コード0のままだと SUCCESS 表示になり気づけない）
    if let Err(e) = result {
        eprintln!("エラー: {:#}", e);
        std::process::exit(1);
    }
    println!("完了！");
}
