# Changelog

## [0.5.0](https://github.com/snowopsdev/selara/compare/v0.4.1...v0.5.0) (2026-09-11)


### Features

* **desktop:** Apply quotation branding ([#104](https://github.com/snowopsdev/selara/issues/104)) ([1afd8a9](https://github.com/snowopsdev/selara/commit/1afd8a9ddd7a530e3e17d1ad3a2dad1066ec011c))


### Bug fixes

* **macos:** Repair shared login, picker startup, and updates ([#102](https://github.com/snowopsdev/selara/issues/102)) ([79a7638](https://github.com/snowopsdev/selara/commit/79a76381cc85e91c5dfae6c863936de278811bc4))
* **signing:** Make temporary signing identities discoverable ([#105](https://github.com/snowopsdev/selara/issues/105)) ([e9aa643](https://github.com/snowopsdev/selara/commit/e9aa6436425f0803a06358c468f7f82ba15fb430))

## [0.4.1](https://github.com/snowopsdev/selara/compare/v0.4.0...v0.4.1) (2026-09-10)


### Bug fixes

* **release:** Recover macOS desktop packaging ([#100](https://github.com/snowopsdev/selara/issues/100)) ([5f8900f](https://github.com/snowopsdev/selara/commit/5f8900f68b96ea109336c46bc9244fffb4a84c7f))

## [0.4.0](https://github.com/snowopsdev/selara/compare/v0.3.0...v0.4.0) (2026-09-10)


### Features

* **config:** Exclude apps from hotkey capture ([#75](https://github.com/snowopsdev/selara/issues/75)) ([8b01021](https://github.com/snowopsdev/selara/commit/8b01021677103a1f87a09f7a15260af9e87e84da))
* **config:** Store API keys in the OS keychain ([#67](https://github.com/snowopsdev/selara/issues/67)) ([955044a](https://github.com/snowopsdev/selara/commit/955044acfefbe6f90909548846667100efb9f72a))
* **core:** Add prompt template variables and a Translate command ([#64](https://github.com/snowopsdev/selara/issues/64)) ([ce07a0a](https://github.com/snowopsdev/selara/commit/ce07a0adf17eb1925fa0f4e466db85266138a8f2))
* **core:** Allow a per-command model override ([#93](https://github.com/snowopsdev/selara/issues/93)) ([f35acba](https://github.com/snowopsdev/selara/commit/f35acba2086f82b77faf69542b3cce0e6a0b6ee5))
* **core:** Per-app command sets ([#97](https://github.com/snowopsdev/selara/issues/97)) ([801c7c3](https://github.com/snowopsdev/selara/commit/801c7c38a9aef13ab55ad0c91f1c8ae070d2a55c))
* **core:** Record local usage and estimated cost ([#80](https://github.com/snowopsdev/selara/issues/80)) ([afa7726](https://github.com/snowopsdev/selara/commit/afa77260ca1b98dfb2ae86798e2e3ab869fc8c90))
* **core:** Warn before sending secret-shaped text to hosted providers ([#78](https://github.com/snowopsdev/selara/issues/78)) ([6ab317a](https://github.com/snowopsdev/selara/commit/6ab317ae78c99a577e32cde6af4d7e3259227f6b))
* **desktop:** Add a Status tab for serve, Accessibility, provider ([#68](https://github.com/snowopsdev/selara/issues/68)) ([77bfba3](https://github.com/snowopsdev/selara/commit/77bfba3581837a2b8701a83c04eb207658f772e7))
* **desktop:** Add auto-update with tauri-plugin-updater ([#77](https://github.com/snowopsdev/selara/issues/77)) ([950b869](https://github.com/snowopsdev/selara/commit/950b8692831507e1450c8cee2db7c840545ec115))
* **desktop:** Add provider presets to the Models tab ([#65](https://github.com/snowopsdev/selara/issues/65)) ([ae26ec1](https://github.com/snowopsdev/selara/commit/ae26ec137ce8dd32626a7bc7d0f314afb2a6f3b6))
* **desktop:** Import and export command packs ([#71](https://github.com/snowopsdev/selara/issues/71)) ([a3de41f](https://github.com/snowopsdev/selara/commit/a3de41f1280afd1c420fd75098c500fe6f4a88dd))
* **desktop:** Supervise serve from the tray and start at login ([#74](https://github.com/snowopsdev/selara/issues/74)) ([60217be](https://github.com/snowopsdev/selara/commit/60217be4865715d01b561ffc76b41663b3f92f6c))
* **providers:** Stream completions into the popup and CLI ([#73](https://github.com/snowopsdev/selara/issues/73)) ([b3aa581](https://github.com/snowopsdev/selara/commit/b3aa5813f7063dc7936e610c5d9cf3f4db6e9812))
* **serve:** Free-form instruction in the picker, save as command ([#76](https://github.com/snowopsdev/selara/issues/76)) ([97fae5a](https://github.com/snowopsdev/selara/commit/97fae5a50d124481b9fb7ea523d949c0fb4c88be))
* **serve:** Keep a history of transformations with copy-back ([#79](https://github.com/snowopsdev/selara/issues/79)) ([65cebaf](https://github.com/snowopsdev/selara/commit/65cebaf69ee600929b5ab69391b8a5acc3c70990))
* **serve:** Keyboard-first picker positioned near the cursor ([#72](https://github.com/snowopsdev/selara/issues/72)) ([7dda998](https://github.com/snowopsdev/selara/commit/7dda998f540752560ed47c2543ff75f9e1075995))
* **serve:** Render markdown and add result actions in the popup ([#66](https://github.com/snowopsdev/selara/issues/66)) ([924712f](https://github.com/snowopsdev/selara/commit/924712f0d8c08a808fd9f9fca820d582dc699bd5))
* **serve:** Run on clipboard when nothing is selected ([#96](https://github.com/snowopsdev/selara/issues/96)) ([606975a](https://github.com/snowopsdev/selara/commit/606975a862c2089d987560244e2277ea2aaf3b92))


### Bug fixes

* **chatgpt:** Keep unknown auth.json fields and make login async ([#57](https://github.com/snowopsdev/selara/issues/57)) ([eaccc4a](https://github.com/snowopsdev/selara/commit/eaccc4a0c19e481e86c55308e1ca3d885c7916fd))
* **config:** Atomic 0600 saves, schema_version, per-section writes ([#58](https://github.com/snowopsdev/selara/issues/58)) ([77b0169](https://github.com/snowopsdev/selara/commit/77b0169d463a1b9e77aa4f4181d035483a91c3aa))
* **core:** Strip chat preambles and fences before Replace ([#54](https://github.com/snowopsdev/selara/issues/54)) ([cd7f855](https://github.com/snowopsdev/selara/commit/cd7f8557ddf46579861371e5b30a9b22dc4b4d30))
* **desktop:** Merge the two OK_STATUS lists into one ([#94](https://github.com/snowopsdev/selara/issues/94)) ([c47ba5f](https://github.com/snowopsdev/selara/commit/c47ba5f1713ccfe8962e00d6d01bcaf7c520a3e0))
* **platform:** Snapshot the pasteboard, guard restore by changeCount ([#63](https://github.com/snowopsdev/selara/issues/63)) ([f189631](https://github.com/snowopsdev/selara/commit/f189631f16eed2d5f6649fd8208f4f2db8caad1c))
* **providers:** Flag truncated output, no temperature for o-series ([#56](https://github.com/snowopsdev/selara/issues/56)) ([9261f70](https://github.com/snowopsdev/selara/commit/9261f7005e6022d957d01fd66331bfda3c2e6e16))
* **serve:** Drop stale job results and add undo for the last replace ([#55](https://github.com/snowopsdev/selara/issues/55)) ([626c47d](https://github.com/snowopsdev/selara/commit/626c47df7499c442ead9712bc156a9aac3c37643))


### Performance

* **providers:** Reuse one HTTP client and retry transient errors ([#59](https://github.com/snowopsdev/selara/issues/59)) ([d3293b8](https://github.com/snowopsdev/selara/commit/d3293b80e3150bc6cacd137c611bf1fe9c79e6d4))
* **serve:** Watch the config file and stop idle repaint polling ([#62](https://github.com/snowopsdev/selara/issues/62)) ([8c6d266](https://github.com/snowopsdev/selara/commit/8c6d266162fb87c83f756bb1723e242e99449efd))

## [0.3.0](https://github.com/snowopsdev/selara/compare/v0.2.1...v0.3.0) (2026-09-05)


### Features

* **skills:** Add E2E QA orchestrator skill ([#9](https://github.com/snowopsdev/selara/issues/9)) ([2cc68c1](https://github.com/snowopsdev/selara/commit/2cc68c1f3744ab7650ac634c377170938df228d1))


### Bug fixes

* **core:** Restore omitted commands and harden provider errors ([#11](https://github.com/snowopsdev/selara/issues/11)) ([a9f9c36](https://github.com/snowopsdev/selara/commit/a9f9c364bcdc80fa96769d3ef6e9b8922e20e0ea))

## [0.2.1](https://github.com/snowopsdev/selara/compare/v0.2.0...v0.2.1) (2026-09-05)


### Bug fixes

* **serve:** Honor size limits on hotkeys and send language hint ([#8](https://github.com/snowopsdev/selara/issues/8)) ([c8d94f2](https://github.com/snowopsdev/selara/commit/c8d94f2414cbcefc347382e7283e037f3185d4cb))


### Documentation

* **readme:** Add Settings screenshots and feature tour ([#6](https://github.com/snowopsdev/selara/issues/6)) ([7840d2d](https://github.com/snowopsdev/selara/commit/7840d2d14709e470793e88075f0145b9b81fb78b))

## [0.2.0](https://github.com/snowopsdev/selara/compare/v0.1.0...v0.2.0) (2026-09-04)


### Features

* **desktop:** Liquid Glass Settings, provider setup, and release automation ([5f42abe](https://github.com/snowopsdev/selara/commit/5f42abeeb4f2d4f521e972f80a4a88e810afeae6))
* **desktop:** Redesign Settings with Liquid Glass styling ([b3229f9](https://github.com/snowopsdev/selara/commit/b3229f9bc2b9d1c81da0ca3b7e540dd3fdb8c5a4))
* **providers:** Add Anthropic and OpenRouter setup with model discovery ([ed24a75](https://github.com/snowopsdev/selara/commit/ed24a75b758ae35d0748fec55ba9894397e81b0e))
