package com.nofakeip.desktop

import android.os.Bundle
import android.view.View
import androidx.activity.enableEdgeToEdge
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat

/**
 * ⚠️ 环境适配补丁 E15（见 doc/11-构建发布与Android.md 第 2.9 节）
 *
 * 这个文件由 tools/build-android.sh 从 tools/android/MainActivity.kt 覆盖写入，
 * 重新执行 `tauri android init` 后会自动重新应用 —— 不要直接改 gen/ 下的副本。
 *
 * 背景：
 *   Android 15（API 35）起，targetSdk >= 35 的应用被强制 edge-to-edge，
 *   无法再通过主题关掉。本项目 targetSdk = 36，窗口默认延伸到状态栏、
 *   手势导航条与刘海区域下方，前端内容会被系统栏压住。
 *
 *   而 **Android WebView 不支持 CSS 的 `env(safe-area-inset-*)`**
 *   （那是 iOS WebKit 的特性），所以只能由原生侧把系统栏尺寸
 *   作为 padding 加到根容器上。
 */
class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    // 必须在 super.onCreate 之前调用（androidx.activity 的要求）
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)

    // android.R.id.content 是承载 WebView 的根容器。
    // decorView 一定存在，用它作为后备，避免个别机型上 content 尚未建立。
    val root: View = window.decorView.findViewById(android.R.id.content) ?: window.decorView

    ViewCompat.setOnApplyWindowInsetsListener(root) { view, insets ->
      val bars = insets.getInsets(
        WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout()
      )
      view.setPadding(bars.left, bars.top, bars.right, bars.bottom)
      // 已经在这里用掉了，别再往子视图传，否则会重复留白
      WindowInsetsCompat.CONSUMED
    }
  }
}
