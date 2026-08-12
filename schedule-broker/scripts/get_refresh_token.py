#!/usr/bin/env python3
"""Google OAuth の refresh_token を取得して .env に書き込む。

ブラウザで承認 -> localhost:8765 でコードを受け取り -> トークン交換 -> .env 更新
まで自動で行う。トークンは端末外に出さない。

使い方:
    cd schedule-broker && python3 scripts/get_refresh_token.py
"""

import http.server
import json
import pathlib
import sys
import threading
import urllib.parse
import urllib.request
import webbrowser

PORT = 8765
REDIRECT_URI = f"http://localhost:{PORT}"
SCOPE = "https://www.googleapis.com/auth/calendar.readonly"
ENV_PATH = pathlib.Path(__file__).resolve().parent.parent / ".env"

received = {}


def load_env() -> dict:
    env = {}
    for line in ENV_PATH.read_text().splitlines():
        if "=" in line and not line.lstrip().startswith("#"):
            k, _, v = line.partition("=")
            env[k.strip()] = v.strip()
    return env


def write_refresh_token(token: str) -> None:
    lines = ENV_PATH.read_text().splitlines()
    out, found = [], False
    for line in lines:
        if line.startswith("GOOGLE_REFRESH_TOKEN="):
            out.append(f"GOOGLE_REFRESH_TOKEN={token}")
            found = True
        else:
            out.append(line)
    if not found:
        out.append(f"GOOGLE_REFRESH_TOKEN={token}")
    ENV_PATH.write_text("\n".join(out) + "\n")


class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        query = urllib.parse.urlparse(self.path).query
        params = urllib.parse.parse_qs(query)

        if "code" in params:
            received["code"] = params["code"][0]
            body = "認証できました。ターミナルに戻ってください。"
        else:
            received["error"] = params.get("error", ["unknown"])[0]
            body = f"認証に失敗しました: {received['error']}"

        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.end_headers()
        self.wfile.write(f"<html><body><h2>{body}</h2></body></html>".encode())

    def log_message(self, *args):
        pass  # アクセスログを出さない


def main() -> int:
    env = load_env()
    client_id = env.get("GOOGLE_CLIENT_ID", "")
    client_secret = env.get("GOOGLE_CLIENT_SECRET", "")

    if not client_id or not client_secret:
        print("GOOGLE_CLIENT_ID / GOOGLE_CLIENT_SECRET が .env にありません", file=sys.stderr)
        return 1

    auth_url = "https://accounts.google.com/o/oauth2/v2/auth?" + urllib.parse.urlencode({
        "client_id": client_id,
        "redirect_uri": REDIRECT_URI,
        "response_type": "code",
        "scope": SCOPE,
        "access_type": "offline",
        "prompt": "consent",
    })

    server = http.server.HTTPServer(("localhost", PORT), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()

    print("ブラウザで承認画面を開きます。")
    print('「このアプリは確認されていません」と出たら 詳細 -> schedule-broker（安全ではないページ）に移動 を選んでください。\n')
    print(f"開かない場合は次のURLを手動で開いてください:\n{auth_url}\n")
    webbrowser.open(auth_url)

    print("承認を待っています...")
    while "code" not in received and "error" not in received:
        pass
    server.shutdown()

    if "error" in received:
        print(f"認証に失敗しました: {received['error']}", file=sys.stderr)
        return 1

    data = urllib.parse.urlencode({
        "client_id": client_id,
        "client_secret": client_secret,
        "code": received["code"],
        "grant_type": "authorization_code",
        "redirect_uri": REDIRECT_URI,
    }).encode()

    try:
        with urllib.request.urlopen(
            urllib.request.Request("https://oauth2.googleapis.com/token", data=data)
        ) as resp:
            token = json.load(resp)
    except urllib.error.HTTPError as e:
        print(f"トークン交換に失敗しました: {e.read().decode()}", file=sys.stderr)
        return 1

    if "refresh_token" not in token:
        print("refresh_token が返りませんでした。", file=sys.stderr)
        print("既に承認済みの場合に起きます。以下で連携を解除してから再実行してください:", file=sys.stderr)
        print("  https://myaccount.google.com/permissions", file=sys.stderr)
        return 1

    write_refresh_token(token["refresh_token"])
    print("\n✅ refresh_token を .env に書き込みました。")
    print("   cargo run でサーバーを起動できます。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
