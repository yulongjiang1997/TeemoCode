import type { SoundEvent } from "@/lib/util/sound";

/** 音效文件存 IndexedDB(localStorage 放 base64 有配额限制,大 mp3 写不进去),
 *  配置(mc.sounds)只存每事件开关。主窗口与桌宠窗口同源,IndexedDB 共享。 */

const DB_NAME = "mc-sounds";
const STORE = "files";

function openDb(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const req = indexedDB.open(DB_NAME, 1);
    req.onupgradeneeded = () => {
      const db = req.result;
      if (!db.objectStoreNames.contains(STORE)) db.createObjectStore(STORE);
    };
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
}

/** 保存事件音效文件(File,含名字);返回文件名(仅显示用)。 */
export async function saveSoundFile(ev: SoundEvent, file: File): Promise<string> {
  const db = await openDb();
  return new Promise((resolve, reject) => {
    const tx = db.transaction(STORE, "readwrite");
    tx.objectStore(STORE).put(file, ev);
    tx.oncomplete = () => {
      db.close();
      resolve(file.name || "");
    };
    tx.onerror = () => {
      db.close();
      reject(tx.error);
    };
  });
}

/** 读取事件音效文件(Blob);无则返回 null。 */
export function getSoundFile(ev: SoundEvent): Promise<Blob | null> {
  return new Promise((resolve) => {
    void (async () => {
      try {
        const db = await openDb();
        const req = db.transaction(STORE, "readonly").objectStore(STORE).get(ev);
        req.onsuccess = () => {
          const blob = (req.result as Blob | undefined) ?? null;
          db.close();
          resolve(blob);
        };
        req.onerror = () => {
          db.close();
          resolve(null);
        };
      } catch {
        resolve(null);
      }
    })();
  });
}

/** 删除事件音效文件。 */
export function deleteSoundFile(ev: SoundEvent): Promise<void> {
  return new Promise((resolve) => {
    void (async () => {
      try {
        const db = await openDb();
        const tx = db.transaction(STORE, "readwrite");
        tx.objectStore(STORE).delete(ev);
        tx.oncomplete = () => {
          db.close();
          resolve();
        };
        tx.onerror = () => {
          db.close();
          resolve();
        };
      } catch {
        resolve();
      }
    })();
  });
}
