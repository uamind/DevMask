<!--
  SPDX-License-Identifier: GPL-3.0-or-later
  Copyright (c) 2025-2026 Seyud
  DevMask: https://github.com/uamind/DevMask
-->

<template>
  <section class="apps-page-skeleton" aria-hidden="true">
    <div class="apps-page-skeleton__head">
      <div class="apps-page-skeleton__title-row">
        <span class="dm-skeleton apps-page-skeleton__title"></span>
        <span class="dm-skeleton apps-page-skeleton__count"></span>
      </div>
      <span class="dm-skeleton apps-page-skeleton__sub"></span>
    </div>

    <div class="apps-page-skeleton__search dm-skeleton"></div>

    <div class="apps-page-skeleton__seg">
      <span v-for="i in 3" :key="i" class="dm-skeleton apps-page-skeleton__seg-item"></span>
    </div>

    <div class="apps-page-skeleton__list">
      <div v-for="i in 6" :key="i" class="apps-page-skeleton__row">
        <span class="dm-skeleton apps-page-skeleton__avatar"></span>
        <span class="apps-page-skeleton__body">
          <span class="dm-skeleton apps-page-skeleton__name"></span>
          <span class="dm-skeleton apps-page-skeleton__package"></span>
        </span>
        <span class="dm-skeleton apps-page-skeleton__tag"></span>
      </div>
    </div>
  </section>
</template>

<style scoped>
.apps-page-skeleton {
  display: flex;
  flex-direction: column;
  width: 100%;
  padding-top: 4px;
}

.apps-page-skeleton__head {
  display: flex;
  flex-direction: column;
  gap: 9px;
  padding: 2px 0 16px;
}

.apps-page-skeleton__title-row {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 10px;
}

.apps-page-skeleton__title {
  width: 84px;
  height: 26px;
  border-radius: 8px;
}

.apps-page-skeleton__count {
  width: 46px;
  height: 11px;
  border-radius: 5px;
}

.apps-page-skeleton__sub {
  width: 72%;
  height: 11px;
  border-radius: 5px;
}

.apps-page-skeleton__search {
  height: 44px;
  border-radius: 14px;
}

.apps-page-skeleton__seg {
  display: flex;
  gap: 3px;
  margin: 12px 0 10px;
  padding: 3px;
  border: 1px solid var(--line);
  border-radius: var(--radius-pill);
}

.apps-page-skeleton__seg-item {
  flex: 1 1 0;
  height: 32px;
  border-radius: var(--radius-pill);
}

.apps-page-skeleton__list {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.apps-page-skeleton__row {
  display: flex;
  align-items: center;
  gap: 11px;
  padding: 12px 13px;
  border: 1px solid var(--line);
  border-radius: var(--radius-tile);
  background: var(--surface);
}

.apps-page-skeleton__avatar {
  width: 38px;
  height: 38px;
  flex: none;
  border-radius: 12px;
}

.apps-page-skeleton__body {
  display: flex;
  flex: 1;
  flex-direction: column;
  gap: 6px;
  min-width: 0;
}

.apps-page-skeleton__name {
  width: 45%;
  height: 12px;
}

.apps-page-skeleton__package {
  width: 62%;
  height: 9px;
}

.apps-page-skeleton__tag {
  width: 62px;
  height: 20px;
  flex: none;
  border-radius: 7px;
}
</style>
