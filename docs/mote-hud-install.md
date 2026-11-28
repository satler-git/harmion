# Mote HUD の導入

## 動的インストール

他の Harmion アプリと同じく、ビルドしたバイナリを `~/.local/bin` に symlink します。

```sh
# 初回だけ。新規ファイルが未追跡だと nix build に含まれない。
git add apps/mote-hud apps/mote/src/lib.rs docs/mote-hud-plan.md docs/mote-hud-install.md

./scripts/install-mote-hud
# installed /home/.../.local/bin/mote-hud -> /nix/store/...-mote-hud-0.1.1/bin/mote-hud
```

`mote-hud` が PATH に入ったら、niri のキーはこれだけです。

```kdl
Mod+S { spawn "mote-hud" "toggle"; }
Mod+Shift+S { spawn "mote-hud" "capture"; }
```

Nix で niri の設定を書いている場合は、binds に次を足します。

```nix
"Mod+S".action.spawn = [ "mote-hud" "toggle" ];
"Mod+Shift+S".action.spawn = [ "mote-hud" "capture" ];
```

`~/.local/bin` が PATH にあることを確認してください。`toggle` はソケットがなければ `--daemon` を自分で起動します。常駐プロセスを別途用意する必要はありません。ソケットは既定で `$XDG_RUNTIME_DIR/mote-hud.sock` です。

パッケージの wrapper は Wayland / Vulkan / Mesa の `LD_LIBRARY_PATH` と、IBM Plex / JetBrains Mono / Noto CJK の `XDG_DATA_DIRS` を設定します。開発ビルドを直接動かすときは、dev shell か次の環境変数が必要です。

```sh
export LD_LIBRARY_PATH=$(pkg-config --variable=libdir wayland-client):$LD_LIBRARY_PATH
```
