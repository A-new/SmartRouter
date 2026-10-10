// English baseline translation (complete reference, fallback for all locales)
import { Translations } from './types';

const en: Translations = {
  'page.freeModels': 'SMART ROUTER',
  'freeModels.fetch': 'Get Free Models',
  'freeModels.fetchHint': 'Get free models from the button above',
  'freeModels.fetchFailed': 'Free models are temporarily unavailable. Try again later.',
  'freeModels.scanning': 'Newly found: ',
  'freeModels.scanConnecting': 'Digging for free models',
  'freeModels.customAdd': 'Custom Add',
  'freeModels.addToRouter': 'Add Model to Smart Router',
  'freeModels.help': 'How It Works',
  'freeModels.help.routerTitle': 'Why use smart routing?',
  'freeModels.help.routerDesc':
    'Free models often run out of quota, hit rate limits, or go down temporarily. Smart routing aggregates every model you have added behind one local API and exposes a single endpoint, so you never have to reconfigure your client.',
  'freeModels.help.routerHow': 'How it works:',
  'freeModels.help.routerPoint1':
    'Shared quota, even load: models under the same key share one quota, and requests are allocated in turn according to the rate limit you set — instead of draining one model until it is throttled and only then switching.',
  'freeModels.help.routerPoint2':
    'Automatic failover: when a model becomes unavailable because of rate limiting, an unpaid balance, an authentication failure, or retirement, it enters a cooldown (about 1 minute for rate limits, 1 hour for auth or billing, 6 hours for a retired model) and the request immediately moves to another available model. The model rejoins routing once the cooldown ends.',
  'freeModels.help.routerPoint3':
    'Auto and group routing: calling with auto picks across every available model, while calling with a group name routes and fails over only inside that group, never across groups. Because platforms name the same model differently, a group keeps a failover on the same model family. Models without a group only take part in auto.',
  'freeModels.help.routerPoint4':
    'Failure reporting: a single request tries at most 20 models. If none of them has quota available, the API returns 429 together with the earliest retry time.',
  'freeModels.help.routerSummary':
    'In short: one endpoint to connect, automatic multi-model scheduling, seamless failover, and maximum use of free quota.',
  'freeModels.help.useTitle': 'How to use it',
  'freeModels.help.step1':
    'Use the external-link icon on a provider card to open its website and obtain an API key.',
  'freeModels.help.step2':
    'Open the provider card, choose a model ID, enter the API key, and save. More providers give the router more free options.',
  'freeModels.help.step3': 'Use Custom Add to connect other free, paid, or private models.',
  'freeModels.docs': 'Details',
  'freeModels.router.emptyTitle': '0 models connected',
  'freeModels.router.emptyDesc': 'Start adding models from the list on the right',
  'freeModels.router.limitReached': 'Smart Router supports up to 20 models',
  'freeModels.router.group': 'Route Group',
  'freeModels.router.groupHint':
    'Models in the same group fail over to each other, even when each platform names them differently. Call the router with this name to use only that group.',
  'freeModels.router.groupReserved': 'That name is reserved by the router.',
  'freeModels.router.groups': 'Groups',
  'freeModels.router.rpm': 'RPM',
  'freeModels.router.rpmHint':
    'Requests per minute this account may spend. Free tiers usually cap one API key around 20, and models sharing a key share that budget. Use -1 for a paid key with no rate limit; leave it empty and the router relies on the advertised limit and on 429 responses.',
  'freeModels.router.rpmInvalid':
    'Requests per minute must be a whole number between 1 and 100000, or -1 for no rate limit. Leave it empty for no declared limit.',
  'freeModels.router.rpmUnlimited': 'Unlimited',
  'freeModels.router.dragHint':
    'Drag a row by its grip to reorder. Candidates that score the same follow this order.',
  'freeModels.router.modelIdHint':
    'Double-click a model ID to edit it in the model configuration dialog.',
  'freeModels.router.resizeHint': 'Drag a column edge to resize it, double-click to reset.',
  'freeModels.router.sortHint': 'Click a column header to sort, click again to reverse the order.',
  'freeModels.router.provider': 'Provider',
  'freeModels.router.status': 'Status',
  'freeModels.router.statusServing': 'Serving',
  'freeModels.router.statusCooling': 'Cooling down',
  'freeModels.router.statusIdle': 'Standby',
  'settings.title': 'SETTINGS',
  'settings.version': 'VERSION',
  'settings.language': 'LANGUAGE',
  'settings.appearance': 'APPEARANCE',
  'settings.themeLight': 'Light',
  'settings.themeDark': 'Dark',
  'settings.themeSystem': 'System',
  'settings.updates': 'UPDATES',
  'settings.latestVersion': 'Already on the latest version',
  'settings.updateTo': 'Update to',
  'settings.updateDownloading': 'Downloading',
  'settings.updateLaunching': 'Launching installer…',
  'settings.closeWindowBehavior': 'CLOSE WINDOW BEHAVIOR',
  'settings.closeDirectly': 'Quit directly',
  'settings.closeToTray': 'Minimize to tray',
  'settings.launchAtStartup': 'Launch at startup',
  'settings.alwaysAsk': 'Always ask',
  'closeDialog.title': 'CLOSE BEHAVIOR',
  'closeDialog.hint':
    'Note: If your ChatGPT / Codex CLI / Claude Desktop / Claude Code has switched models, quitting will make it unusable.',
  'btn.addModel': 'ADD MODEL',
  'btn.close': 'Close',
  'btn.copied': 'Copied',
  'btn.remove': 'REMOVE',
  'model.name': 'Name',
  'model.apiKey': 'API Key',
  'model.paste': 'Paste',
  'model.modelId': 'Model ID',
  'model.modelIdPlaceholder': 'Search or enter model ID…',
  'model.openaiUrl': 'OpenAI URL',
  'model.anthropicUrl': 'Anthropic URL',
  'model.requiredFields': 'Enter a name, model ID, API key, and an OpenAI or Anthropic URL.',
  'model.invalidOpenaiUrl': 'Enter a valid OpenAI URL.',
  'model.invalidAnthropicUrl': 'Enter a valid Anthropic URL.',
  'model.editConfig': 'EDIT MODEL CONFIG',
  'model.escCancel': 'CANCEL',
  'model.enterSave': 'SAVE',
  'key.encrypted': 'Encrypted globally — API Key will self-destruct if environment changes',
  'key.destroyed': 'Environment change detected — API Key has self-destructed, please re-enter',
  'error.requestFailed': 'Request failed, please try again.',
};
export { en };
