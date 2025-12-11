# macOSでのKoradarテスト手順

## 前提条件

1. **Docker Desktop**がインストールされ、実行中であること
2. **Rust**と**Cargo**がインストールされていること
3. **Trunk**がインストールされていること（`cargo install trunk`）

## テスト手順

### ステップ1: セットアップ確認

```bash
cd koradar
./scripts/check_docker_setup.sh
```

必要なコンポーネントが揃っているか確認します。

### ステップ2: サーバーとフロントエンドの起動

**ターミナル1**でサーバーを起動：

```bash
cd koradar
make run
```

ブラウザで `http://localhost:3000` を開きます。3ペインのUI（Registers、Trace、Memory）が表示されます。

### ステップ3A: システムエミュレーションモードでのテスト（簡単）

**ターミナル2**で：

```bash
cd koradar
make trace
```

**注意**: "No bootable device" が表示されますが、これは正常です。システムエミュレーションモードでは、カーネル/ディスクイメージが必要です。

**確認事項**:
- ブラウザのTraceパネルに `{"Init":{"vcpu_index":0}}` が表示される
- プラグインが正常にロードされていることを確認

### ステップ3B: 実際のバイナリをトレース（Docker使用）

#### 3B-1: テストバイナリの作成

```bash
cd koradar
make test-binary
```

これで `/tmp/koradar_test_hello` が作成されます（x86_64 Linuxバイナリ）。

#### 3B-2: QEMUをDocker内でビルド（初回のみ、10-30分かかります）

```bash
cd koradar
./scripts/setup_qemu_docker.sh
```

**注意**: このステップは時間がかかります。完了まで待ってください。

#### 3B-3: バイナリをトレース

**ターミナル2**で（サーバーはターミナル1で実行中）：

```bash
cd koradar
./scripts/trace_docker.sh /tmp/koradar_test_hello
```

**確認事項**:
- ブラウザのTraceパネルに実行イベントが表示される
- レジスタとメモリの値が更新される
- スライダーで時間を移動できる

### ステップ4: UIでの操作確認

ブラウザで以下を確認：

1. **スライダー操作**: 時間を前後に移動
2. **Step Forward/Backward**: 1ステップずつ移動
3. **レジスタ表示**: 各レジスタの値が表示される
4. **メモリ表示**: メモリの内容が16進数とASCIIで表示される
5. **トレースログ**: 実行イベントがリアルタイムで表示される

## トラブルシューティング

### Dockerが起動していない

```bash
# Docker Desktopを起動
open -a Docker
```

### QEMU (Docker) がビルドされていない

```bash
cd koradar
./scripts/setup_qemu_docker.sh
```

### サーバーが起動しない

```bash
# ポート3000が使用中でないか確認
lsof -i :3000

# 既存のプロセスを停止
pkill -f koradar-server
```

### フロントエンドが表示されない

```bash
# フロントエンドを再ビルド
cd koradar
make build-frontend
```

## 制限事項

macOSでは以下の制限があります：

1. **Linuxバイナリの直接トレース不可**: Dockerが必要
2. **QEMU User Mode**: macOSではビルド不可
3. **パフォーマンス**: Docker使用時は若干のオーバーヘッドあり

## 推奨事項

完全なテストには、Linux環境（VMまたはリモートサーバー）の使用を推奨します：

- Linuxでは `make trace BINARY=/path/to/binary` で直接トレース可能
- パフォーマンスが向上
- セットアップが簡単

## 次のステップ

テストが成功したら：

1. より複雑なバイナリでテスト
2. タイムレスナビゲーション機能の確認
3. メモリ/レジスタの状態復元の確認

