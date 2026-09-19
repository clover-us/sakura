#!/usr/bin/env node
/**
 * sync-animation-pool.mjs —— 从上游 config.jsonc 抽取「动画池」并更新本项目的默认配置模板。
 *
 * 为什么用脚本而不是手抄：
 *   动画池里有 100+ 个动画名，手抄必然出错（少一个名字就是运行时 404）。
 *   本脚本只做**结构化抽取 + 定向替换**，并把结果写回本仓库的
 *   `config/default-config.jsonc`，保证与上游池内容逐项一致。
 *
 * 用法：node scripts/sync-animation-pool.mjs [上游仓库路径]
 *   默认读取 D:\programs\github\whale-pet（**只读**，绝不修改上游）
 *
 * 替换策略（刻意保守）：
 *   本项目模板里以 `// >>> 动画池（由 scripts/sync-animation-pool.mjs 同步）` 与
 *   `// <<< 动画池` 为界；脚本只替换这两个标记之间的内容，其余注释与字段一律不动。
 */
import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

const upstreamRepo = process.argv[2] || 'D:\\programs\\github\\whale-pet';
const upstreamConfig = join(upstreamRepo, 'dsh-pet', 'assets', 'config.jsonc');
const targetConfig = join(process.cwd(), 'config', 'default-config.jsonc');

const BEGIN = '// >>> 动画池（由 scripts/sync-animation-pool.mjs 同步）';
const END = '// <<< 动画池';

/** 剥掉 JSONC 注释（保留字符串里的 // 与 /*，规则与本项目 Rust 侧一致） */
function stripJsonc(input) {
  let out = '';
  let inString = false;
  let escaped = false;
  for (let i = 0; i < input.length; i += 1) {
    const c = input[i];
    if (inString) {
      out += c;
      if (escaped) escaped = false;
      else if (c === '\\') escaped = true;
      else if (c === '"') inString = false;
      continue;
    }
    if (c === '"') {
      inString = true;
      out += c;
      continue;
    }
    if (c === '/' && input[i + 1] === '/') {
      while (i < input.length && input[i] !== '\n') i += 1;
      out += '\n';
      continue;
    }
    if (c === '/' && input[i + 1] === '*') {
      i += 2;
      let prev = '';
      while (i < input.length && !(prev === '*' && input[i] === '/')) {
        if (input[i] === '\n') out += '\n';
        prev = input[i];
        i += 1;
      }
      continue;
    }
    out += c;
  }
  return out;
}

const upstream = JSON.parse(stripJsonc(readFileSync(upstreamConfig, 'utf8')));

// ---- 校验上游池的形状（宁可在这里报错，也不要把坏数据写进模板）----
const anim = upstream.animations;
if (!anim || !Array.isArray(anim.idle) || !Array.isArray(anim.turn) || !Array.isArray(anim.clicks)) {
  throw new Error('上游 config.jsonc 的 animations 段形状不符合预期（idle/turn/clicks 必须是数组）');
}
if (!Array.isArray(anim.moves?.actions) || !Array.isArray(anim.categories)) {
  throw new Error('上游 animations 缺少 moves.actions 或 categories');
}
if (!upstream.animationWeights) throw new Error('上游 config.jsonc 缺少 animationWeights');

/** 统计池里的动作总数（用于打印与自检） */
const categoryActionCount = anim.categories.reduce((sum, c) => sum + c.actions.length, 0);

/**
 * 生成写进模板的 JSON 片段。
 *
 * 两个**必须遵守**的约束（都踩过坑）：
 *   1. 必须是"平铺的键值块"而不是嵌套对象：这段内容会被插进配置根对象的内部，
 *      早期版本用 JSON.stringify 整个对象，结果在根里插进一个"裸块"，配置直接语法错误；
 *   2. 块自身**既不能以逗号开头、也不能以逗号结尾**：
 *      开头不能有逗号是因为注释剥离会把标记行换成空行（孤立逗号 → `key must be a string`）；
 *      结尾不能有逗号是因为本块是根对象的最后一段（尾随逗号 → `trailing comma`）。
 *      连接用的逗号由模板放在 `pets` 之后（`],`）。
 */
function buildBlock() {
  const body = JSON.stringify(
    {
      animations: {
        idle: anim.idle,
        turn: anim.turn,
        drag: anim.drag,
        clicks: anim.clicks,
        moves: anim.moves,
        categories: anim.categories,
        events: anim.events,
      },
      animationWeights: upstream.animationWeights,
    },
    null,
    2,
  );
  // 缩进两级并去掉最外层大括号，让它与配置里的其它键同级。
  // **结尾不加逗号**：本块是配置根对象的最后一段（其后紧跟根 `}`），
  // 加逗号会变成 JSON 的尾随逗号（实测报 trailing comma）。
  // 而块**开头**也不需要逗号——逗号由模板放在 `pets` 之后（见模板里的 `],`）。
  const lines = body.split('\n');
  const inner = lines.slice(1, -1).map((line) => `  ${line}`);
  return inner.join('\n');
}

const block = buildBlock();

const template = readFileSync(targetConfig, 'utf8');
const beginAt = template.indexOf(BEGIN);
const endAt = template.indexOf(END);
if (beginAt < 0 || endAt < 0 || endAt < beginAt) {
  throw new Error(`模板里找不到同步标记，请先补上：\n${BEGIN}\n${END}`);
}

// 保留标记行本身，只替换它们之间的内容
const before = template.slice(0, template.indexOf('\n', beginAt) + 1);
const after = template.slice(endAt);
const updated = `${before}${block}\n${after}`;
writeFileSync(targetConfig, updated, 'utf8');

console.log('[同步] 上游池已写入模板 config/default-config.jsonc');
console.log(`[同步] idle=${anim.idle.length} turn=${anim.turn.length} drag=${anim.drag.length} ` +
  `clicks=${anim.clicks.length} moves=${anim.moves.actions.length} ` +
  `categories=${anim.categories.length}（含 ${categoryActionCount} 个动作）`);
console.log(`[同步] 顶层权重 idle=${upstream.animationWeights.idle} turn=${upstream.animationWeights.turn} ` +
  `move=${upstream.animationWeights.move}`);
