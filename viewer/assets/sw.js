// var cacheName = 'egui-template-pwa';
// var filesToCache = [
//     './',
//     './index.html',
//     './eframe_template.js',
//     './eframe_template_bg.wasm',
// ];

// /* Start the service worker and cache all of the app's content */
// self.addEventListener('install', function (e) {
//     e.waitUntil(
//         caches.open(cacheName).then(function (cache) {
//             return cache.addAll(filesToCache);
//         })
//     );
// });

// /* Serve cached content when offline */
// self.addEventListener('fetch', function (e) {
//     e.respondWith(
//         caches.match(e.request).then(function (response) {
//             return response || fetch(e.request);
//         })
//     );
// });

let directoryHandle = null;
let fileCache = {};
function setDirectoryHandle(handle) {
    if (directoryHandle) {
        directoryHandle.close();
        clearCache();
    }
    directoryHandle = handle;
}
function clearCache() {
    fileCache.values().forEach(handle => {
        handle.close();
    });
    fileCache = {};
}

async function entryExists(path) {
    if (fileCache[path]) {
        return true;
    }

    const pathParts = path.split('/');
    let currentHandle = directoryHandle;

    // Traverse subdirectories if needed.
    for (let i = 0; i < pathParts.length - 1; i++) {
        currentHandle = await currentHandle.getDirectoryHandle(pathParts[i]);
    }

    return (await Array.fromAsync(currentHandle.keys())).includes(pathParts[pathParts.length - 1]);
}

// Works on both files and directories
async function getFileHandle(path) {
    if (fileCache[path]) {
        return true;
    }
    const pathParts = path.split('/');
    let currentHandle = directoryHandle;

    // Traverse subdirectories if needed.
    for (let i = 0; i < pathParts.length - 1; i++) {
        currentHandle = await currentHandle.getDirectoryHandle(pathParts[i]);
    }

    // Get the file handle and file
    const fileHandle = await currentHandle.getFileHandle(pathParts[pathParts.length - 1]);

    const file = await fileHandle.createSyncAccessHandle({ mode: "read-only" });
    fileCache[path] = file;
    return file;
}

self.addEventListener('message', async (event) => {
    const { type, data } = event.data;

    try {
        if (!type) {
            throw new Error('No type provided');
        }
        if (!data) {
            throw new Error('No data provided');
        }

        if (type == 'cleanup') {
            setDirectoryHandle(null);
            event.source.postMessage({ success: true });
        }

        if (type === 'set-directory') {
            if (!(data instanceof FileSystemDirectoryHandle)) {
                throw new Error('Invalid directory handle');
            }

            setDirectoryHandle(data);
            event.source.postMessage({ success: true });
        }

        if (type == 'entry-exists') {
            if (!directoryHandle) {
                throw new Error('No directory handle set');
            }

            const result = await entryExists(data);
            event.source.postMessage({ success: true, exists: result });
        }

        if (type === 'get-file-size') {
            if (!directoryHandle) {
                throw new Error('No directory handle set');
            }

            const fileHandle = await getFileHandle(data);
            if (!fileHandle) {
                throw new Error('File not found');
            }

            const fileSize = fileHandle.getSize();
            event.source.postMessage({ success: true, size: fileSize });
        }

        if (type === 'read-file-all') {
            if (!directoryHandle) {
                throw new Error('No directory handle set');
            }

            const fileHandle = await getFileHandle(data);
            if (!fileHandle) {
                throw new Error('File not found');
            }

            const fileSize = fileHandle.getSize();
            const buffer = new ArrayBuffer(fileSize);
            fileHandle.read(buffer);

            event.source.postMessage({ success: true, buffer });
        }

        if (type === 'read-file-at') {
            if (!directoryHandle) {
                throw new Error('No directory handle set');
            }

            let { path, buffer, offset } = data;

            const fileHandle = await getFileHandle(path);
            if (!fileHandle) {
                throw new Error('File not found');
            }


            let bytes_read = await fileHandle.read(buffer, { at: offset });

            event.source.postMessage({ success: true, bytes_read });
        }
    }
    catch (error) {
        event.source.postMessage({ error });
    }
});