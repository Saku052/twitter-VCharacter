#!/usr/bin/env python3
"""TickTick の access_token を取得して .env に書き込む。

ブラウザで承認 -> localhost:8766 でコードを受け取り -> トークン交換 -> .env 更新
まで自動で行う。トークンは端末外に出ない。

使い方:
    cd schedule-broker && python3 scripts/get_ticktick_token.py
"""

import base64
import http.server
import json
import pathlib
import sys
import threading
import urllib.error
import urllib.parse
import urllib.request
import webbrowser

PORT = 8766
REDIRECT_URI = f"http://localhost:{PORT}"
SCOPE = "tasks:read tasks:write"
ENV_PATH = pathlib.Path(__file__).resolve().parent.parent / ".env"

received = {}


def load_env() -> dict:
    env = {}
    for line in ENV_PATH.read_text().splitlines():
        if "=" in line and not line.lstrip().startswith("#"):
            k, _, v = line.partition("=")
            env[k.strip()] = v.strip()
    return env


def write_env(key: str, value: str) -> None:
    lines = ENV_PATH.read_text().splitlines()
    out, found = [], False
    for line in lines:
        if line.startswith(f"{key}="):
            out.append(f"{key}={value}")
            found = True
        else:
            out.append(line)
    if not found:
        out.append(f"{key}={value}")
    ENV_PATH.write_text("\n".join(out) + "\n")


class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        params = urllib.parse.parse_qs(urllib.parse.urlparse(self.path).query)

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
        pass


def main() -> int:
    env = load_env()
    client_id = env.get("TICKTICK_CLIENT_ID", "")
    client_secret = env.get("TICKTICK_CLIENT_SECRET", "")

    if not client_id or not client_secret:
        print("TICKTICK_CLIENT_ID / TICKTICK_CLIENT_SECRET が .env にありません", file=sys.stderr)
        return 1

    auth_url = "https://ticktick.com/oauth/authorize?" + urllib.parse.urlencode({
        "client_id": client_id,
        "scope": SCOPE,
        "state": "schedule-broker",
        "redirect_uri": REDIRECT_URI,
        "response_type": "code",
    })

    server = http.server.HTTPServer(("localhost", PORT), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()

    print("ブラウザで承認画面を開きます。\n")
    print(f"開かない場合は次のURLを手動で開いてください:\n{auth_url}\n")
    print("※ redirect_uri のエラーが出る場合は、TickTick開発者センターの")
    print(f"   OAuth redirect URL に {REDIRECT_URI} が保存されているか確認してください。\n")
    webbrowser.open(auth_url)

    print("承認を待っています...")
    while "code" not in received and "error" not in received:
        pass
    server.shutdown()

    if "error" in received:
        print(f"認証に失敗しました: {received['error']}", file=sys.stderr)
        return 1

    # client_id/secret は Basic 認証ヘッダで送る（TickTickの仕様）
    basic = base64.b64encode(f"{client_id}:{client_secret}".encode()).decode()
    data = urllib.parse.urlencode({
        "code": received["code"],
        "grant_type": "authorization_code",
        "scope": SCOPE,
        "redirect_uri": REDIRECT_URI,
    }).encode()

    req = urllib.request.Request(
        "https://ticktick.com/oauth/token",
        data=data,
        headers={
            "Authorization": f"Basic {basic}",
            "Content-Type": "application/x-www-form-urlencoded",
        },
    )

    try:
        with urllib.request.urlopen(req) as resp:
            token = json.load(resp)
    except urllib.error.HTTPError as e:
        print(f"トークン交換に失敗しました: {e.code} {e.read().decode()}", file=sys.stderr)
        return 1

    if "access_token" not in token:
        print(f"access_token が返りませんでした: {token}", file=sys.stderr)
        return 1

    write_env("TICKTICK_ACCESS_TOKEN", token["access_token"])
    print("\n✅ TICKTICK_ACCESS_TOKEN を .env に書き込みました。")

    # 登録先プロジェクトの候補を表示する（IDのみ、内容は出さない）
    try:
        req = urllib.request.Request(
            "https://api.ticktick.com/open/v1/project",
            headers={"Authorization": f"Bearer {token['access_token']}"},
        )
        with urllib.request.urlopen(req) as resp:
            projects = json.load(resp)
        print(f"\n利用可能なプロジェクト（{len(projects)}件）:")
        for p in projects:
            print(f"  {p.get('name')}  id={p.get('id')}")
        print("\n特定のリストに入れたい場合は TICKTICK_PROJECT_ID に上記IDを設定してください。")
        print("未設定なら先頭のプロジェクトが使われます。")
    except Exception as e:
        print(f"（プロジェクト一覧の取得はスキップしました: {e}）")

    return 0


if __name__ == "__main__":
    sys.exit(main())
