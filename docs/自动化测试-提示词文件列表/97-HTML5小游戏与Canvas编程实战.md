# 自动化测试提示词 — HTML5 小游戏与 Canvas 编程实战（CO01–CO10）

> 使用说明见同目录 `README.md`。每条提示词在同一 Session 内按轮次顺序喂入。
>
> 编号段 CO01–CO10 · 聚焦「浏览器里的小游戏」：游戏循环与后台节流 / 离屏分层渲染 / 雪碧图与资源管理 / 输入与虚拟摇杆 / 场景管理 / Web Audio 音效 / 摄像机与视差卷轴 / HUD 与像素完美 / 微信小游戏适配 / 性能与内存排查

## 维度说明
本维度考察 Agent 在**浏览器 Canvas 平台**做小游戏工程的实战能力。所有任务固定 4 轮，要求至少 1 轮 Bash 执行并断言、至少 1 轮 Write 落盘产物到 `tmpPlan/agent-test/` 沙盒，工具链限于 Bash/Read/Write + node（**node 跑 JS 模拟 Canvas 行为**）+ python3（**PIL 合成像素图作为最终渲染断言**）。所有路径前缀 `tmpPlan/agent-test/`，输入数据现场生成，**产物只允许在 `tmpPlan/agent-test/` 沙盒内**。衡量重点：把抽象的浏览器游戏工程概念（rAF/分层/雪碧图/摇杆/场景栈/Audio/摄像机/HUD/小游戏/性能）转化为可由 node 脚本或 PIL 像素图验证的工程产物。

---

### CO01 游戏循环：rAF、deltaTime 与后台节流
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-10_06-批量测试优化与状态标记方案.md）
- **预期档位**: medium
- **考察维度**: 浏览器游戏循环 + 时间步 + 标签页生命周期
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write 一份 Node 脚本 `tmpPlan/agent-test/game_loop.js`：实现 `requestAnimationFrame` 模拟循环（用 `setImmediate` 推进帧）、每帧 `dt = (now - last) / 1000` clamp 到 50ms、累加器固定 60Hz 逻辑步；同时输出 200 帧的 `dt` 与 `tick` 序列到 `tmpPlan/agent-test/loop_log.json`。
  2. Bash 跑：`node tmpPlan/agent-test/game_loop.js`，再 `python3 -c "import json; d=json.load(open('tmpPlan/agent-test/loop_log.json')); assert max(e['dt'] for e in d)<=0.05, 'dt 未 clamp'; assert sum(1 for e in d if e.get('tick'))>=190, 'tick 不足'; print('PASS frames=',len(d))"` 断言 dt 全 ≤ 50ms 且 tick 数 ≥ 190。
  3. Read `loop_log.json` 的前 5 帧后 Write 第二个 Node 脚本 `tmpPlan/agent-test/visibility_sim.js`：模拟「切后台 3 秒→切回」，验证回切时 dt 不暴涨（仍然 ≤ 50ms 而不是 3 秒），输出 `tmpPlan/agent-test/visibility_log.json`。
  4. Bash 复检：`node tmpPlan/agent-test/visibility_sim.js` 后 `python3 -c "import json; d=json.load(open('tmpPlan/agent-test/visibility_log.json')); assert max(e['dt'] for e in d)<=0.05; assert d[-1]['phase']=='resumed'; print('PASS')"`。

### CO02 离屏 Canvas 与分层渲染
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-10_06-批量测试优化与状态标记方案.md）
- **预期档位**: medium
- **考察维度**: 图层分离 + 离屏缓存 + drawImage 合成
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write Node 脚本 `tmpPlan/agent-test/layers.js`：用 `canvas`（如未装则改纯 JS 模拟二维数组 buffer）实现三图层分离（背景层 map[200][200] 静态、实体层 entities[100] 动态、UI 层 ui[10]），记录每帧的 drawcall 数与渲染时间到 `tmpPlan/agent-test/layers_log.json`。
  2. Bash 跑：`node tmpPlan/agent-test/layers.js` → 输出 100 帧日志，再 `python3 -c "import json; d=json.load(open('tmpPlan/agent-test/layers_log.json')); assert all(f['bg_draws']==1 for f in d), '背景层应只画 1 次'; assert all(f['ui_draws']<=10 for f in d); print('PASS avg_draws=',sum(f['total'] for f in d)//len(d))"`。
  3. Read 日志，Write `LayerCache` 类的测试 `tmpPlan/agent-test/cache_test.js`：验证「失效标记」行为——连续两次同 key 渲染只调一次底层；输出命中/未命中计数到 `tmpPlan/agent-test/cache_log.json`。
  4. Bash 跑：`node tmpPlan/agent-test/cache_test.js` 后 `python3 -c "import json; d=json.load(open('tmpPlan/agent-test/cache_log.json')); assert d['miss']==1 and d['hit']>=10; print('PASS hit_rate=',d['hit']/(d['hit']+d['miss']))"`。

### CO03 雪碧图动画与资源预载管理
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-10_06-批量测试优化与状态标记方案.md）
- **预期档位**: medium
- **考察维度**: 帧动画 + 资源清单 + 预载进度
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write Node 脚本 `tmpPlan/agent-test/sprite_anim.js`：用 PIL 现场生成 8 帧 32×32 雪碧图存 `tmpPlan/agent-test/sheet.png`，脚本读取并按 `frame = floor(t/120ms) % 8` 抽取当前帧写入 `tmpPlan/agent-test/current_frame.png`（先跑 PIL 生成图，再跑 node 读帧生成目标）。
  2. Bash 跑：`python3 -c "from PIL import Image; im=Image.new('RGBA',(256,32),(255,0,0,255)); [im.paste((i*30%255,i*50%255,128,255),(i*32,0,32,32)) for i in range(8)]; im.save('tmpPlan/agent-test/sheet.png')"` → `node tmpPlan/agent-test/sprite_anim.js` → 输出 `current_frame.png`。
  3. Read `current_frame.png` 的像素用 PIL 验证：`python3 -c "from PIL import Image; im=Image.open('tmpPlan/agent-test/current_frame.png'); px=im.getpixel((16,16)); print('frame0_color=',px); assert px[0]<10 and px[1]<10, '首帧应为接近红色'"`。
  4. Write 资源加载器 `tmpPlan/agent-test/asset_loader.js`：JSON 清单 `{sheets:['sheet.png'],maps:['map.json']}` 用 `Promise.all` 并行加载，模拟 `img.decode()` 等待，输出加载进度到 `tmpPlan/agent-test/loader_log.json`；Bash 跑后断言 `progress_100` 必须出现且单资源失败不阻塞。

### CO04 输入系统：键盘、触屏与虚拟摇杆
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-10_06-批量测试优化与状态标记方案.md）
- **预期档位**: medium
- **考察维度**: 输入抽象 + 多点触控 + 虚拟摇杆实现
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write Node 脚本 `tmpPlan/agent-test/input_state.js`：实现键盘 `Set<string>` 状态表、模拟 60 帧序列按键事件（左按下 10 帧→同时按右 5 帧→全松开），每帧输出当前 down 集合到 `tmpPlan/agent-test/input_log.json`。
  2. Bash 跑：`node tmpPlan/agent-test/input_state.js` 后 `python3 -c "import json; d=json.load(open('tmpPlan/agent-test/input_log.json')); assert d[10]['down']==['ArrowLeft']; assert d[15]['down']==['ArrowLeft','ArrowRight']; assert d[20]['down']==[]; print('PASS frames=',len(d))"`。
  3. Read 日志后 Write 虚拟摇杆 `tmpPlan/agent-test/joystick.js`：触摸起点 (200,300)、半径 r=50，模拟 6 次拖动（向右 30/向上 40/对角 35,-35/边界 70/松手）输出归一化方向序列到 `tmpPlan/agent-test/joy_log.json`。
  4. Bash 跑：`node tmpPlan/agent-test/joystick.js` 后 `python3 -c "import json; d=json.load(open('tmpPlan/agent-test/joy_log.json')); import math; assert all(math.hypot(v['dx'],v['dy'])<=1.001 for v in d); assert d[0]['dx']>0.5 and d[0]['dy']<0.1; print('PASS samples=',len(d))"`。

### CO05 场景管理与过渡动画
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-10_06-批量测试优化与状态标记方案.md）
- **预期档位**: medium
- **考察维度**: 场景栈 FSM + 过渡效果 + 暂停恢复
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write Node 脚本 `tmpPlan/agent-test/scene_stack.js`：实现 `SceneManager` 栈（menu/game/pause 三场景）、模拟事件序列 push(game)/push(pause)/pop()/replace(gameOver) 共 10 步，每步输出栈快照与当前 update 调用的场景名到 `tmpPlan/agent-test/scene_log.json`。
  2. Bash 跑：`node tmpPlan/agent-test/scene_stack.js` 后 `python3 -c "import json; d=json.load(open('tmpPlan/agent-test/scene_log.json')); assert [s['current'] for s in d]==['menu','game','pause','game','gameOver',...] or len(d)>=8; assert all(s['frozen_under'] for s in d if s['current']=='pause'); print('PASS stack_depth_max=',max(s['depth'] for s in d))"`。
  3. Read 日志后 Write 过渡动画 `tmpPlan/agent-test/transition.js`：黑屏淡入淡出（alpha 0→1→0 共 60 帧），用 PIL 把每帧 alpha 渲到灰度 PNG 序列到 `tmpPlan/agent-test/frames/`。
  4. Bash 跑：`node tmpPlan/agent-test/transition.js` → 对中间帧 `frames/030.png` 用 `python3 -c "from PIL import Image; im=Image.open('tmpPlan/agent-test/frames/030.png'); px=im.getpixel((50,50)); assert 100<px[0]<160, '过渡中段应为中灰'; print('mid_alpha=',px[0])"` 断言。

### CO06 Web Audio 音效与背景音乐
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-10_06-批量测试优化与状态标记方案.md）
- **预期档位**: medium~hard
- **考察维度**: AudioContext 解锁 + 音效池 + 无缝循环 BGM
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write Node 脚本 `tmpPlan/agent-test/audio_bus.js`：实现 `AudioBus` 单例 + 解锁前静默丢弃、解锁后正常播放（用伪 AudioContext 对象模拟），模拟 50 次 play('jump') 并发请求，输出实际播放实例数与丢弃数到 `tmpPlan/agent-test/audio_log.json`。
  2. Bash 跑：`node tmpPlan/agent-test/audio_bus.js` 后 `python3 -c "import json; d=json.load(open('tmpPlan/agent-test/audio_log.json')); assert d['played']<=4; assert d['dropped']==46; assert d['unlocked_after']>0; print('PASS played=',d['played'],'dropped=',d['dropped'])"` 断言同时实例数 ≤ 4。
  3. Read 日志后 Write BGM 调度器 `tmpPlan/agent-test/bgm_loop.js`：120 BPM、4/4 拍，按 16 小节 lookahead 调度 `start(when)`，输出调度时间表到 `tmpPlan/agent-test/bgm_log.json`。
  4. Bash 跑：`node tmpPlan/agent-test/bgm_loop.js` 后 `python3 -c "import json; d=json.load(open('tmpPlan/agent-test/bgm_log.json')); assert len(d)==64; gaps=[d[i+1]['when']-d[i]['end'] for i in range(len(d)-1)]; assert all(-0.001<g<0.01 for g in gaps), '存在间隙'; print('PASS seamless_bar=',64)"` 断言每小节无缝衔接。

### CO07 摄像机系统与视差卷轴
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-10_06-批量测试优化与状态标记方案.md）
- **预期档位**: medium
- **考察维度**: 摄像机跟随 + 死区与前视 + 多层视差
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write Node 脚本 `tmpPlan/agent-test/camera.js`：实现摄像机跟随 + 死区（±40px 不动）+ 平滑 `cam += (target - cam) * min(1, 6*dt)`，模拟玩家从 x=0 走到 x=500 共 600 帧，输出 cam 与 target 序列到 `tmpPlan/agent-test/cam_log.json`。
  2. Bash 跑：`node tmpPlan/agent-test/camera.js` 后 `python3 -c "import json; d=json.load(open('tmpPlan/agent-test/cam_log.json')); dead=[i for i,e in enumerate(d) if abs(e['target']-e['cam'])<40]; assert len(dead)>=200, '死区帧应充足'; assert all(abs(d[i+1]['cam']-d[i]['cam'])<=20 for i in range(len(d)-1)); print('PASS dead_frames=',len(dead))"`。
  3. Read 日志后 Write 视差卷轴 `tmpPlan/agent-test/parallax.js`：远景 0.2 / 中景 0.5 / 玩家 1.0 / 前景 1.3 四层，每帧输出每层 x 偏移到 `tmpPlan/agent-test/parallax_log.json`。
  4. Bash 跑：`node tmpPlan/agent-test/parallax.js` 后用 PIL 把四层偏移渲成单张测试图 `tmpPlan/agent-test/parallax.png`（每层不同灰度），再 `python3 -c "from PIL import Image; im=Image.open('tmpPlan/agent-test/parallax.png'); assert im.size[0]==400; assert len(set(im.crop((0,y,400,y+1)).tobytes() for y in range(4)))==4; print('PASS 4 layers rendered')"`。

### CO08 HUD、UI 与像素完美渲染
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-10_06-批量测试优化与状态标记方案.md）
- **预期档位**: medium
- **考察维度**: Canvas 内 UI vs DOM 层 + 像素画缩放 + 自适应
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write PIL 脚本 `tmpPlan/agent-test/hud_render.py`：画一张 480×270 HUD 测试图（血条 100→60→30 三段、分数 9999、连击 x5 浮字），保存到 `tmpPlan/agent-test/hud.png`。
  2. Bash 跑：`python3 tmpPlan/agent-test/hud_render.py` 后 `python3 -c "from PIL import Image; im=Image.open('tmpPlan/agent-test/hud.png'); assert im.size==(480,270); red=sum(1 for px in im.crop((10,10,210,30)).getdata() if px[0]>200 and px[1]<50); assert red>1000, '血条应有红色像素'; print('PASS red_px=',red)"`。
  3. Read HUD 图后 Write 像素完美缩放脚本 `tmpPlan/agent-test/pixel_scale.py`：480×270 源图整数倍 ×2 放大到 960×540（image.NEAREST），再 ×1.5 非整数放大到 720×405（image.BILINEAR），保存到 `tmpPlan/agent-test/scaled_int.png` 与 `tmpPlan/agent-test/scaled_nonint.png`。
  4. Bash 跑：`python3 tmpPlan/agent-test/pixel_scale.py` 后断言整数倍仍锐利、非整数糊：`python3 -c "from PIL import Image; a=Image.open('tmpPlan/agent-test/scaled_int.png'); b=Image.open('tmpPlan/agent-test/scaled_nonint.png'); assert a.size==(960,540); assert b.size==(720,405); import os; assert a.tobytes()!=b.resize((960,540)).tobytes(); print('PASS int_sharp nonint_blur')"`。

### CO09 微信小游戏适配与发布
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-10_06-批量测试优化与状态标记方案.md）
- **预期档位**: medium~hard
- **考察维度**: 平台差异层 + 包体分包 + 开放数据域 + 审核发布
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write Node 脚本 `tmpPlan/agent-test/platform_adapter.js`：实现 `PlatformAdapter` 接口（createCanvas/loadImage/playAudio/getStorage），Web 实现 + 微信小游戏 stub 实现各一份，统一调用入口 `gameLoop.run()` 输出每个 API 在两平台分别走了哪个实现到 `tmpPlan/agent-test/platform_log.json`。
  2. Bash 跑：`node tmpPlan/agent-test/platform_adapter.js` 后 `python3 -c "import json; d=json.load(open('tmpPlan/agent-test/platform_log.json')); assert all('impl' in e for e in d); web=[e for e in d if e['platform']=='web']; mp=[e for e in d if e['platform']=='minigame']; assert len(web)==len(mp)==5; print('PASS api_count=',len(d))"`。
  3. Read 日志后 Write 资源分包加载器 `tmpPlan/agent-test/subpkg_loader.js`：主包 ≤ 4MB 限制、模拟 20 个资源（首屏 3 / 远程 17），输出加载顺序与累计大小到 `tmpPlan/agent-test/subpkg_log.json`。
  4. Bash 跑：`node tmpPlan/agent-test/subpkg_loader.js` 后 `python3 -c "import json; d=json.load(open('tmpPlan/agent-test/subpkg_log.json')); first=[e for e in d if e['phase']=='first_screen']; assert len(first)==3; assert max(e['cumulative'] for e in d)<=4*1024*1024; print('PASS main_pkg_max=',max(e['cumulative'] for e in d))"` 断言首屏资源先于远程加载且主包累计 ≤ 4MB。

### CO10 性能与内存排查实战
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-10_06-批量测试优化与状态标记方案.md）
- **预期档位**: hard
- **考察维度**: DevTools 性能面板 + GC 抖动 + 帧率埋点
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write Node 脚本 `tmpPlan/agent-test/perf_naive.js`：每帧 new 100 个粒子对象（无对象池），模拟 1000 帧，输出每帧分配对象数与 GC 触发次数到 `tmpPlan/agent-test/perf_naive.json`。
  2. Bash 跑：`node tmpPlan/agent-test/perf_naive.js` 后 `python3 -c "import json; d=json.load(open('tmpPlan/agent-test/perf_naive.json')); assert all(e['alloc']==100 for e in d); assert d[-1]['gc_count']>=5; print('naive_gc=',d[-1]['gc_count'])"`。
  3. Read 后 Write 对象池版本 `tmpPlan/agent-test/perf_pooled.js`：复用 100 个粒子对象，输出同样指标到 `tmpPlan/agent-test/perf_pooled.json`。
  4. Bash 跑：`node tmpPlan/agent-test/perf_pooled.js` 后断言：`python3 -c "import json; a=json.load(open('tmpPlan/agent-test/perf_naive.json')); b=json.load(open('tmpPlan/agent-test/perf_pooled.json')); assert all(e['alloc']==0 for e in b); assert b[-1]['gc_count']<a[-1]['gc_count']; print('PASS gc_reduction=',a[-1]['gc_count']-b[-1]['gc_count'])"` 断言对象池版 GC 次数显著降低。
