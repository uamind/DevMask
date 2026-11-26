<!--
  SPDX-License-Identifier: GPL-3.0-or-later
  Copyright (c) 2025-2026 Seyud
  DevMask: https://github.com/uamind/DevMask
-->

<template>
  <div class="net-row">
    <span class="net-row__k">{{ label }}</span>
    <span :class="['net-row__v', { 'net-row__v--mono': mono }]">{{ value }}</span>
  </div>
</template>

<script setup lang="ts">
/**
 * 网络页的键值行基元。
 *
 * 状态区 / 中继 / 探测结果三处的行排版完全一致，抽出来避免三份复制；
 * 值一律靠右、长值换行（IPv6、错误原因不会被截断）。
 */
withDefaults(
  defineProps<{
    label: string
    value: string
    /** 版本、settings 值、端口等用等宽（数值对齐，扫读更快）。 */
    mono?: boolean
  }>(),
  { mono: false }
)
</script>
