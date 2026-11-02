---
title: Extended features
tags: [tables, tasks]
---

# Extended Markdown

## Tables

| Feature | Status | Notes |
|:--------|:------:|------:|
| Tables | done | right-aligned |
| Task lists | done | `inline code` works |
| **Bold** cell | partial | 42 |

A wide table that needs shrinking at narrow widths:

| Name | Description |
|------|-------------|
| mmore | A pager like more, with Markdown rendering and syntax highlighting |
| less | The classic pager with many more features than more |

## Task list

- [x] Render tables
- [ ] Render footnotes with a long description that wraps
- Regular bullet in the same list

## Alerts

> [!NOTE]
> Useful information that users should know.

> [!CAUTION]
> Negative consequences of an action.

## Footnotes

Markdown has footnotes.[^note] And more.[^2]

[^note]: The first footnote.
[^2]: The second footnote, which is a bit longer so it wraps at narrow widths.

## Images and badges

![The mmore logo](docs/logo.png)

[![CI status](https://ci.example.com/badge.svg)](https://ci.example.com)
