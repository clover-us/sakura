/**
 * 桌宠窗口的前端入口。
 *
 * 职责边界（刻意保持很薄）：
 *   1. 取配置（Rust 侧已完成校验与素材解析，前端不做二次兜底）；
 *   2. 建事件通道：光标采样（穿透自愈 + 拖拽跟随）、显示器几何（抛掷边界）、窗口位置；
 *   3. 装配 PetRuntime 并把上述事件接进去；
 *   4. 出错**大声报错**（页面红色错误条 + 诊断日志），与上游"绝不静默兜底"的约定一致。
 */
import { invoke, listen, readPetEnvironment } from './bridge/tauri.ts';
import { petLog, petLogError, setLogLabel } from './bridge/log.ts';
import type { CursorSample, DisplaysSample, PetConfig, PetRuntime as PetRuntimeState } from './bridge/contract.ts';
import { requireElement, showFatalError } from './renderer/dom.ts';
import { CursorChannel } from './renderer/cursor.ts';
import { PetRuntime } from './renderer/runtime.ts';

/** 取本窗口的宠物配置（窗口标签由 URL 的 ?label= 给出，见 Rust 侧 pet_window.rs 建窗处） */
async function loadPetConfig(label: string): Promise<PetConfig> {
  // Rust 侧把注入的 assetBaseUrl 一并带在配置里返回，前端不必自行拼平台相关的 URL
  return invoke<PetConfig>('get_pet_config', { label });
}

/** 页面启动流程 */
async function bootstrap(): Promise<void> {
  try {
    // 环境注入检查放在最前：拿不到就直接报错，避免后面出现难以定位的资源加载失败
    const env = readPetEnvironment();

    const label = new URLSearchParams(window.location.search).get('label');
    if (!label) throw new Error('窗口 URL 缺少 ?label=（无法确定本窗口承载哪只宠物）');
    setLogLabel(label);
    petLog(`启动: 页面加载完成，assetBaseUrl=${env.assetBaseUrl}`);

    const config = await loadPetConfig(label);
    // 注入的资源根地址是唯一权威（Rust 侧知道本平台的协议形式）
    const petConfig: PetConfig = { ...config, assetBaseUrl: env.assetBaseUrl };
    petLog(
      `配置: size=${petConfig.size} idle="${petConfig.idle}" click="${petConfig.click || '(无)'}" ` +
        `hitBox=${petConfig.hitBox.x.toFixed(0)},${petConfig.hitBox.y.toFixed(0)} ` +
        `${petConfig.hitBox.width.toFixed(0)}x${petConfig.hitBox.height.toFixed(0)}`,
    );

    const dom = {
      box: requireElement<HTMLElement>('#pet-box'),
      videoA: requireElement<HTMLVideoElement>('#pet-video-a'),
      videoB: requireElement<HTMLVideoElement>('#pet-video-b'),
      hit: requireElement<HTMLElement>('#pet-hit'),
    };

    // 运行时的引用需要提前交给光标通道的回调，故先用一个占位变量接住
    let runtime: PetRuntime | null = null;
    const cursor = new CursorChannel(
      (event, handler) => listen<CursorSample>(event, handler),
      (frame) => runtime?.onCursorFrame(frame),
    );

    runtime = new PetRuntime(petConfig, dom, { cursor });

    // 窗口位置：Rust 是唯一真相，首次同步与后续每次移动都走同一事件
    await listen<PetRuntimeState>('pet://window-state', (state) => runtime?.setWindowState(state));
    // 右键菜单的动作：菜单是**独立小窗**（见 src/menu-page.ts），点了菜单项之后由宿主转到这里执行
    // ——动画链、气泡锚点、回角落都在宠物页手里，菜单页只负责"报出用户点了什么"。
    await listen<{ anim: string | null; action: string | null }>('pet://menu-action', (action) => {
      void runtime?.onMenuAction(action);
    });
    // 碎碎念（M3）：宿主按周期生成一句话，这里负责"说"——弹气泡（10 秒）+ 可选播一条 whisper 动画。
    // 生成在宿主侧（Rust），因为周期、多宠物轮换、失败退避都只在那一处；页面只管表现。
    await listen<{ petLabel: string; text: string }>('pet://whisper', (payload) => {
      if (payload.petLabel !== label) return;
      void runtime?.onWhisper(payload.text);
    });
    // 当前窗口位置主动拉一次（事件可能早于本模块注册，避免首帧位置为空）
    runtime.setWindowState(await invoke<PetRuntimeState>('get_pet_runtime', { label }));

    await listen<DisplaysSample>('pet://displays', (sample) => {
      petLog(`显示器: 几何变化，${sample.areas.length} 块屏`);
      runtime?.onDisplays(sample);
    });
    // 首帧几何也主动拉一次（抛掷边界用；事件可能在启动瞬间还没到）
    try {
      const displays = await invoke<DisplaysSample>('get_displays');
      petLog(
        `显示器: 首帧几何 工作区 ${displays.areas.length} 块 面板 ${displays.panels.length} 块 ` +
          `主屏=${displays.primary.width}x${displays.primary.height}`,
      );
      runtime.onDisplays(displays);
    } catch (err) {
      petLogError('几何: 首帧拉取失败（抛掷边界暂用兜底值）', err);
    }

    await runtime.start();
    petLog('启动: 运行时装配完成，开始接收光标采样');

    // 诊断自测钩子（仅在 URL 带 ?autotest=N 时触发；见 PetRuntime 的自测方法）
    const autotest = new URLSearchParams(window.location.search).get('autotest');
    if (autotest === '1') {
      petLog('启动: 检测到 autotest=1，开始受控拖拽 + 甩抛自测');
      // 自测期间屏蔽真实命中判定：真实指针的位置与合成轨迹无关，否则会把合成序列打断
      void runtime.runSelfTest();
    } else if (autotest === '2') {
      petLog('启动: 检测到 autotest=2，开始"拖到命中区外松手"失控场景自测');
      void runtime.runDetachTest();
    } else if (autotest === '3') {
      petLog('启动: 检测到 autotest=3，启动坐标链与命中判定的实时探针');
      // 30 秒（1800 帧）：默认的 3 秒太短，来不及在外部挪几次光标做对照
      runtime.startProbe(1800);
    } else if (autotest === '6') {
      // 气泡验证：确认独立气泡窗能显示、位置正确、且在时长后自动隐藏
      petLog('启动: 检测到 autotest=6，2.5 秒后显示一句测试气泡');
      window.setTimeout(() => runtime.say('气泡独立小窗自测：这条文字不应该挡住下层点击', 5000), 2500);
    } else if (autotest === '7') {
      // 气泡穿透长驻模式：只为"进程外探针"服务（`scripts/probe-window-styles.ps1`）。
      // 与 autotest=6 的差别只有一个：气泡保持 60 秒，让外部脚本有足够时间反复采样
      // 窗口样式位与命中归属。应用正常运行路径与 6 完全一致（同一个 say()）。
      petLog('启动: 检测到 autotest=7，2.5 秒后显示长驻测试气泡（60 秒，供进程外探针采样）');
      window.setTimeout(() => runtime.say('气泡穿透长驻自测：进程外探针在这段时间里反复采样', 60000), 2500);
    } else if (autotest === '8') {
      // 气泡"不吃点击"的**真点击**验证：把气泡压在宠物身上，等外部脚本在重叠点打真鼠标点击。
      // 读完 `runBubbleOverlapTest` 的注释就知道为什么必须重叠才测得出来。
      petLog('启动: 检测到 autotest=8，2.5 秒后把气泡压在宠物身上（等外部真点击）');
      window.setTimeout(() => runtime.runBubbleOverlapTest(), 2500);
    } else if (autotest === '9') {
      // 气泡复用自测：显示 → 3 秒后自动隐藏 → 再显示一次。
      // 断言点在宿主日志：`气泡窗已创建` 只出现一次（第二次是**复用**同一个窗口），
      // 且第二次的 `气泡窗显示完成` 里样式标签仍有 T（穿透位没被隐藏/显示动作抹掉）。
      petLog('启动: 检测到 autotest=9，气泡"显示→隐藏→再显示"，验证窗口复用与穿透位保持');
      window.setTimeout(() => runtime.say('气泡复用自测：第一句，3 秒后自动隐藏', 3000), 2500);
      window.setTimeout(() => runtime.say('气泡复用自测：第二句，验证同一个窗口被复用', 30000), 9000);
    } else if (autotest === '10') {
      // 重复甩出自测：每轮"甩出去 → 在空中抓住 → 再甩一次"，断言每次松手都真的起飞。
      // 对应 bug：`flying` 标志在"空中被抓住"路径漏复位 → 只有第一次能甩（见 runRepeatThrowTest）
      petLog('启动: 检测到 autotest=10，开始"重复甩出 / 空中重甩"自测');
      void runtime.runRepeatThrowTest();
    } else if (autotest === '5') {
      // 漫游验证：漫游权重只有 5%，自动化时等到它太慢；这里确定性触发一次行走
      petLog('启动: 检测到 autotest=5，强制走一段以验证漫游');
      window.setTimeout(() => void runtime.forceWalk(), 2500);
    } else if (autotest === '4') {
      // 注入式拖拽验证：把按键位交给自测控制，让宿主注入的合成拖拽能完整跑完
      petLog('启动: 检测到 autotest=4，进入合成输入保持模式');
      runtime.holdSyntheticButton(3000);
    }

    // 页面卸载时主动释放媒体与监听，避免 WebView2 侧悬挂解码器
    window.addEventListener('beforeunload', () => {
      petLog('退出: 页面卸载，释放媒体与监听');
      runtime?.dispose();
    });
  } catch (err) {
    petLogError('启动失败', err);
    showFatalError(err instanceof Error ? err.message : String(err));
  }
}

// 未捕获异常也记进日志：透明窗口里这类错误在界面上完全不可见
window.addEventListener('error', (event) => {
  petLogError('未捕获异常', event.message);
});
window.addEventListener('unhandledrejection', (event) => {
  petLogError('未处理的 Promise 拒绝', event.reason);
});

void bootstrap();
