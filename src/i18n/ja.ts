// Japanese translation
// UI tone: polite-form (です/ます) but compact; tech terms stay in katakana/English
// where natural (API / GitHub / Claude / Codex / etc.)
import { Translations } from './types';
const ja: Partial<Translations> = {
  'page.freeModels': 'スマートルーター',
  'freeModels.fetch': '無料モデルを取得',
  'freeModels.fetchHint': '上のボタンから無料モデルを取得してください',
  'freeModels.fetchFailed': '無料モデルを取得できません。しばらくしてから再試行してください。',
  'freeModels.scanning': '新しく発見：',
  'freeModels.scanConnecting': '無料モデルを探索中',
  'freeModels.customAdd': 'カスタム追加',
  'freeModels.addToRouter': 'スマートルーターにモデルを追加',
  'freeModels.help': '使い方',
  'freeModels.help.routerTitle': 'スマートルーティングが必要な理由',
  'freeModels.help.routerDesc':
    '無料モデルは、利用枠の枯渇、レート制限、一時的な障害がよく起こります。スマートルーターは追加済みのモデルを 1 つのローカル API に集約し、外部には常に 1 つの API アドレスだけを提供するため、設定を頻繁に切り替える必要がありません。',
  'freeModels.help.routerHow': '動作は次のとおりです。',
  'freeModels.help.routerPoint1':
    '利用枠を共有して均等に呼び出す：同じ Key の複数モデルは 1 つの利用枠を共有し、設定したレート制限に従ってリクエストを順番に割り当てます。1 つのモデルが制限に達するまで使い続けてから切り替える、ということはありません。',
  'freeModels.help.routerPoint2':
    '自動フェイルオーバー：モデルがレート制限、未払い、認証失敗、提供終了などで利用できなくなると、クールダウンに入り（レート制限は約 1 分、認証・未払いは 1 時間、提供終了は 6 時間）、ただちに他の利用可能なモデルへ切り替えます。クールダウンが終わると自動的にルーティングへ復帰します。',
  'freeModels.help.routerPoint3':
    '自動ルーティングとグループルーティング：auto で呼び出すと利用可能なすべてのモデルから選択し、グループ名で呼び出すとそのグループ内だけでルーティングとフェイルオーバーを行い、グループをまたぎません。同じモデルでもプラットフォームごとに名前が異なるため、グループを使えばフェイルオーバー後も同じモデル系統を使い続けられます。グループ未設定のモデルは auto にのみ参加します。',
  'freeModels.help.routerPoint4':
    '失敗時の応答：1 回のリクエストで試すモデルは最大 20 個です。すべてのモデルに利用可能な枠がない場合は、429 と最早の再試行可能時刻を返します。',
  'freeModels.help.routerSummary':
    'つまり、1 つのアドレスで接続し、複数モデルを自動で振り分け、障害時も意識せず切り替わり、無料枠を最大限に活用できます。',
  'freeModels.help.useTitle': '使用方法',
  'freeModels.help.step1':
    'プロバイダーカード右端の外部リンクアイコンからサイトを開き、API Key を取得します。',
  'freeModels.help.step2':
    'プロバイダーカードを開き、モデル ID を選び、API Key を入力して保存します。プロバイダーが多いほど、無料の選択肢が増えます。',
  'freeModels.help.step3':
    '「カスタム追加」から、その他の無料、有料、プライベートモデルも接続できます。',
  'freeModels.docs': '詳細',
  'freeModels.router.emptyTitle': '接続済みモデルは 0 件です',
  'freeModels.router.emptyDesc': '右側のリストからモデルを追加してください',
  'freeModels.router.limitReached': 'スマートルーターには最大 20 モデルまで追加できます',
  'freeModels.router.group': 'ルートグループ',
  'freeModels.router.groupHint':
    '同じグループのモデルは互いにフォールバックします。プラットフォームごとにモデル名が違っても同じグループにできます。この名前でルーターを呼ぶと、そのグループだけを使用します。',
  'freeModels.router.groupReserved': 'その名前はルーターが予約しています。',
  'freeModels.router.groups': 'グループ',
  'freeModels.router.rpm': 'RPM',
  'freeModels.router.rpmHint':
    'このアカウントで 1 分あたりに使えるリクエスト数。無料枠は API キー単位で 20 前後に制限されることが多く、同じキーのモデルはその枠を共有します。有料キーで制限なしにする場合は -1 を入力してください。空欄にするとプロバイダーの申告値と 429 応答に委ねます。',
  'freeModels.router.rpmInvalid':
    '1 分あたりのリクエスト数は 1〜100000 の整数、または制限なしを表す -1 で入力してください。空欄なら未申告になります。',
  'freeModels.router.rpmUnlimited': '制限なし',
  'freeModels.router.dragHint':
    'グリップをドラッグして行を並べ替えます。同じ評価の候補はこの順序で優先されます。',
  'freeModels.router.modelIdHint':
    'モデル ID をダブルクリックすると、設定ダイアログで編集できます。',
  'freeModels.router.resizeHint':
    '列の端をドラッグして幅を変更、ダブルクリックで既定値に戻します。',
  'freeModels.router.sortHint': '列見出しをクリックで並べ替え、もう一度クリックで逆順。',
  'freeModels.router.provider': 'プロバイダー',
  'freeModels.router.status': '状態',
  'freeModels.router.statusServing': '応答中',
  'freeModels.router.statusCooling': 'クールダウン中',
  'freeModels.router.statusIdle': '待機',
  'settings.title': '設定',
  'settings.version': 'バージョン',
  'settings.language': '言語',
  'settings.appearance': '外観',
  'settings.themeLight': 'ライト',
  'settings.themeDark': 'ダーク',
  'settings.themeSystem': 'システムに従う',
  'settings.updates': 'アップデート',
  'settings.latestVersion': '最新版を使用中です',
  'settings.updateTo': 'アップデート',
  'settings.updateDownloading': 'ダウンロード中',
  'settings.updateLaunching': 'インストーラーを起動中…',
  'settings.closeWindowBehavior': 'ウィンドウを閉じる動作',
  'settings.closeDirectly': '直接終了',
  'settings.closeToTray': 'トレイに最小化',
  'settings.alwaysAsk': '毎回確認',
  'settings.launchAtStartup': '起動時に開く',
  'closeDialog.title': 'ウィンドウの閉じ方',
  'closeDialog.hint':
    '注意：ChatGPT / Codex CLI / Claude デスクトップ版 / Claude Code のモデルを切り替えている場合、終了すると使用できなくなります。',
  'btn.addModel': 'モデル追加',
  'btn.close': '閉じる',
  'btn.copied': 'コピーしました',
  'btn.remove': '除去',
  'model.name': '名前',
  'model.apiKey': 'API キー',
  'model.paste': '貼り付け',
  'model.modelId': 'モデル ID',
  'model.modelIdPlaceholder': 'モデル ID を検索または入力…',
  'model.openaiUrl': 'OpenAI URL',
  'model.anthropicUrl': 'Anthropic URL',
  'model.requiredFields':
    '名前、モデル ID、API キー、および OpenAI か Anthropic の URL を入力してください。',
  'model.invalidOpenaiUrl': '有効な OpenAI URL を入力してください。',
  'model.invalidAnthropicUrl': '有効な Anthropic URL を入力してください。',
  'model.editConfig': 'モデル設定を編集',
  'model.escCancel': 'キャンセル',
  'model.enterSave': '保存',
  'key.encrypted': '全体暗号化済み — 環境が変化すると API キーは自動消去されます',
  'key.destroyed': '環境の変化を検出。API キーが自動消去されました。再度入力してください',
  'error.requestFailed': 'リクエストに失敗しました。もう一度お試しください。',
};
export default ja;
