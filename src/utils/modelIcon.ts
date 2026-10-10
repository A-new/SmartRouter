// Smart icon detection — match model name/ID to icon file
export const getModelIcon = (name: string, modelId?: string): string | null => {
  const nameText = name.toLowerCase();
  const modelIdText = (modelId || '').toLowerCase();

  // Provider/directory rows do not pass a real modelId. In that case the
  // vendor name must win over model-family words embedded in the title.
  if (!modelIdText) {
    if (['ccvibe', 'cc vibe', 'cc-vibe'].some((kw) => nameText.includes(kw))) {
      return './icons/models/ccvibe.png';
    }
  }

  const text = `${nameText} ${modelIdText}`;

  // Matching rules: keywords -> icon file
  const iconMap: [string[], string][] = [
    // Qwen model (紫色): model id (qwen3.8-max-preview etc.) or「通义」.
    // MUST precede the platform rule — the platform row's text is CJK「千问」
    // (no `qwen` token), and the token-plan URL carries no `qwen` substring, so
    // this rule only fires when a real qwen modelId is present (config tab).
    [['qwen', '通义', 'tongyi'], 'qwen'],
    // Qwen platform (白底): platform name「千问AI平台」or token-plan/bailian
    // host. Catches the usage-tab URL and the right-panel provider row, which
    // have no modelId and thus miss the model rule above.
    [['千问', 'qianwenai', 'maas.aliyuncs', 'bailian'], 'qianwen'],
    [['claude', 'anthropic', 'sonnet', 'opus', 'haiku'], 'claude'],
    [['gpt', 'openai', 'chatgpt', 'o1', 'o3'], 'chatgpt'],
    [['gemma'], 'google'],
    [['gemini', 'palm'], 'gemini'],
    [['deepseek'], 'deepseek'],
    [['mistral', 'mixtral'], 'mistral'],
    [['minimax'], 'minimax'],
    [['grok', 'x.ai'], 'grok'],
    [['groq'], 'groq'],
    [['cerebras'], 'cerebras'],
    [['modelscope'], 'modelscope'],
    [['ollama'], 'ollama'],
    [['sambanova', 'samba nova'], 'sambanova'],
    [['scaleway'], 'scaleway'],
    [['typhoon', 'opentyphoon'], 'typhoon'],
    [['w&b', 'wandb', 'weights & biases'], 'wandb'],
    [['kimi', 'moonshot'], 'kimi'],
    [['longcat', '美团', 'meituan', '龙猫'], 'longcat'],
    [['glm', 'zhipu', '智谱', 'z.ai', 'bigmodel'], 'glm'],
    [['ernie', 'wenxin', '文心'], 'ernie'],
    [['hunyuan', '混元'], 'hunyuan'],
    [['cohere', 'command'], 'cohere'],
    [['perplexity', 'pplx'], 'perplexity'],
    [['together'], 'together'],
    [['volcengine', 'volces', '火山', 'ark.cn-beijing'], 'volcengine'],
    [['byteplus', 'bytepluses'], 'byteplus'],
    [['doubao', '豆包', 'bytedance'], 'bytedance'],
    [['xiaomi', '小米', 'mimo'], 'xiaomi'],
    [['nemotron', 'nvidia'], 'nemotron'],
    [['stepfun', 'step', '阶跃'], 'stepfun'],
    [['granite', 'ibm'], 'granite'],
    [['meta'], 'meta'],
    [['openrouter'], 'openrouter'],
    [['agnes'], 'agnes'],
    // OpenCode (Zen / Go) — a gateway hosting many vendors' models, matched
    // by the provider-row name. The model brand in the id must win, so this
    // stays below the model-brand rules.
    [['opencode', 'open code'], 'opencode'],
    // Resellers (pure compute providers that host third-party models, e.g.
    // CC Vibe) go LAST. A model card carries a modelId that identifies the
    // actual model brand (glm/kimi/deepseek/minimax), and the model logo must
    // win. The vendor logo only matches when the modelId has no recognized
    // brand — such as provider rows, which pass modelId=''. Model ID and vendor
    // are separate concerns; do not move resellers above model brands.
    [['ccvibe', 'cc vibe', 'cc-vibe'], 'ccvibe'],
    // 88API — token aggregation relay hosting many model brands; matches on
    // its own name only, so it never outranks a recognized model brand above.
    [['88api'], '88api'],
    // APIMart — API image/video relay; own-name match only (model brands win).
    [['apimart', 'api mart'], 'apimart'],
  ];

  for (const [keywords, icon] of iconMap) {
    if (keywords.some((kw) => text.includes(kw))) {
      if (icon === 'agnes') return './icons/models/agnes.png';
      if (icon === 'ccvibe') return './icons/models/ccvibe.png';
      if (icon === '88api') return './icons/models/88api.png';
      if (icon === 'apimart') return './icons/models/apimart.png';
      if (icon === 'byteplus') return './icons/models/byteplus.png';
      if (icon === 'qianwen') return './icons/models/qianwen.png';
      return `./icons/models/${icon}.svg`;
    }
  }
  return null;
};
