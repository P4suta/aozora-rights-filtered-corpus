# aozora rights-filtered corpus

青空文庫の公開テキストから、機械的かつ保守的な権利基準をすべて通過した版だけを固定する非公式コーパスです。2026-08-02 snapshotは4,417版を収録します。本文は合成fixtureではなく、公式テキストZIPを固定した青空文庫mirror commitから展開し、Shift_JISからUTF-8へ変換したものです。

このリポジトリは青空文庫および各関係者による公式サービスではありません。判定基準は日本での表示と米国でのhostingを想定した保守的な運用基準であり、全世界の法域について権利を保証するものではありません。

## 採用基準

版identityは `作品ID + テキストZIP URL` です。次の条件をすべて満たす版だけを `corpus/manifest.json` に採用します。

- 作品とCSVに現れる全関係者の著作権フラグが `なし`
- 初出が空でなく、記載された全発表年を曖昧なく解析できる
- 2026年snapshotでは最新発表年が1930年以前（`cutoff = reference year - 96`）
- 図書カードとテキストZIPが青空文庫公式HTTPS URL
- 固定上流commitのZIPを安全に展開でき、宣言文字コードからUTF-8へ損失なく変換できる
- CSV、ZIP、UTF-8本文のSHA-256を再計算できる

曖昧な初出、相対年、メタデータ不一致、不正URL、未対応文字コードなどは理由付きで `corpus/quarantine.json` に隔離します。作品別allowlistはありません。採用数を増やす変更は、一般化できる判定器の改善としてレビューします。

入力は[青空文庫公式拡張CSV](https://www.aozora.gr.jp/index_pages/list_person_all_extended_utf8.zip)と[青空文庫site mirror](https://github.com/aozorabunko/aozorabunko)の40桁commitに固定されています。

## 再現

裏側はRustだけで実装し、workflowの手続きはshell scriptではなく `xtask` に集約しています。

```console
cargo xtask doctor
cargo xtask check
cargo xtask scan
cargo xtask snapshot --jobs 20
cargo xtask verify
```

`snapshot` は既定で固定commitのraw fileを取得します。一度上流mirrorを取得した環境では、次のようにネットワークなしで同じsnapshotを再生成できます。

```console
cargo xtask snapshot --upstream-root ../aozorabunko --jobs 20
```

`--upstream-root` はcheckoutの `HEAD` が `corpus.toml` の固定commitと完全一致しなければ失敗します。生成後の `verify` はmanifest順序、identity、全権利条件、URL、全digest、UTF-8、欠落・余分な本文をfail-closedで検査します。

## 更新

日次workflowは上流HEAD、基準日、cutoff、CSV hashを更新し、全件を再生成します。

```console
cargo xtask refresh --reference-date 2026-08-02 --jobs 20
```

更新は追加・除外・本文変更・隔離理由・before/after snapshot rootを `corpus/update-report.md` に出し、PRを作るだけです。自動merge、自動baseline更新、作品別例外は行いません。前回採用数から25版を超える減少はPR作成前に停止します。

## 利用上の注意

generatorと保守コードはMIT Licenseです。青空文庫由来の書誌情報・本文をMITで再ライセンスするものではありません。本文を利用する前に[青空文庫収録ファイルの取り扱い規準](https://www.aozora.gr.jp/guide/kijyunn.html)と各entryの出典・権利根拠を確認してください。
