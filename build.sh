#!/usr/bin/env bash
# 构建 Pong 示例到 platforms/wechat/package/。
#
# 真正的构建逻辑在适配层里（addons/bevy-wechat/tools/build.sh），这里只把
# 本仓库的目录布局翻译成它的参数。额外参数会原样传给适配层，例如：
#
#   ./build.sh --no-minify
#   INITIAL_MEMORY_BYTES=$((32*1024*1024)) ./build.sh
#
# 微信小游戏的主包必须绑定你自己的小游戏 AppID：
#
#   WECHAT_APPID=<your-mini-game-appid> ./build.sh
set -euo pipefail
cd "$(dirname "$0")"

APPID=${WECHAT_APPID:-}
if [ -z "$APPID" ]; then
  echo "缺少 WECHAT_APPID：微信小游戏必须用小游戏 AppID 打开和预览（touristappid 不生效）。" >&2
  echo "用法：WECHAT_APPID=<your-mini-game-appid> ./build.sh" >&2
  exit 1
fi

ADAPTER=addons/bevy-wechat

if [ ! -f "$ADAPTER/tools/build.sh" ]; then
  echo "找不到适配层：$ADAPTER/tools/build.sh" >&2
  echo "先执行：git submodule update --init --recursive" >&2
  exit 1
fi

exec "$ADAPTER/tools/build.sh" \
  --crate platforms/wechat \
  --out platforms/wechat/package \
  --template "$ADAPTER/template" \
  --appid "$APPID" \
  --title "bevy-pong" \
  --project "bevy-wechat-example" \
  --name pong \
  --wasm pong_wechat \
  "$@"
