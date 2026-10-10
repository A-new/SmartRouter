# 图标目录说明

## 目录结构

```
public/icons/
└── models/     # AI 模型图标
```

## 图标规范

### 模型图标 (models/)

由 `src/utils/modelIcon.ts` 按模型名 / modelId 关键字动态匹配
（``./icons/models/${icon}.svg``），新增品牌需同时在该文件的 iconMap 中登记。

- **格式**: SVG（推荐）或 PNG
- **尺寸**: 建议 64x64 或 128x128 像素
- **命名**: 全小写，使用连字符分隔

## 使用方式

统一调用 `getModelIcon(name, modelId)`，不要硬编码图标路径。
