// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

import { createRouter, createWebHistory, type RouteRecordRaw } from 'vue-router'

const routes: RouteRecordRaw[] = [
  {
    path: '/',
    name: 'dashboard',
    component: () => import('./pages/DashboardPage.vue'),
    meta: { title: '设备总览', subtitle: '所有设备、配置漂移与 Relay 状态' },
  },
  {
    path: '/devices',
    name: 'devices',
    component: () => import('./pages/DevicesPage.vue'),
    meta: { title: '设备', subtitle: '添加、删除和同步 DevMask 设备' },
  },
  {
    path: '/proxies',
    name: 'proxies',
    component: () => import('./pages/ProxiesPage.vue'),
    meta: { title: '代理池', subtitle: '出口代理定义、绑定关系与出口 IP 探测' },
  },
  {
    path: '/apps',
    name: 'apps',
    component: () => import('./pages/AppsPage.vue'),
    meta: { title: '应用矩阵', subtitle: '同一应用在不同设备上的最终机型' },
  },
  {
    path: '/batch',
    name: 'batch',
    component: () => import('./pages/BatchPage.vue'),
    meta: { title: '批量配置', subtitle: '一次把统一机型下发给多台设备' },
  },
  {
    path: '/relay',
    name: 'relay',
    component: () => import('./pages/RelayPage.vue'),
    meta: { title: '接入 AI', subtitle: 'AI 只需要对接一个 Relay 入口' },
  },
  {
    path: '/logs',
    name: 'logs',
    component: () => import('./pages/LogsPage.vue'),
    meta: { title: '操作日志', subtitle: '添加、删除、同步和批量配置记录' },
  },
  { path: '/:pathMatch(.*)*', redirect: '/' },
]

export const router = createRouter({
  history: createWebHistory(),
  routes,
  scrollBehavior: () => ({ top: 0 }),
})
