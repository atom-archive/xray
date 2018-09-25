(function webpackUniversalModuleDefinition(root, factory) {
	if(typeof exports === 'object' && typeof module === 'object')
		module.exports = factory();
	else if(typeof define === 'function' && define.amd)
		define([], factory);
	else if(typeof exports === 'object')
		exports["memo"] = factory();
	else
		root["memo"] = factory();
})(global, function() {
return /******/ (function(modules) { // webpackBootstrap
/******/ 	// The module cache
/******/ 	var installedModules = {};
/******/
/******/ 	// object to store loaded chunks
/******/ 	// "0" means "already loaded"
/******/ 	var installedChunks = {
/******/ 		"main": 0
/******/ 	};
/******/
/******/ 	// object to store loaded and loading wasm modules
/******/ 	var installedWasmModules = {};
/******/
/******/ 	function promiseResolve() { return Promise.resolve(); }
/******/
/******/ 	var wasmImportObjects = {
/******/ 		"./dist/memo_wasm_bg.wasm": function() {
/******/ 			return {
/******/ 				"./memo_wasm": {
/******/ 					"__wbindgen_throw": function(p0i32,p1i32) {
/******/ 						return installedModules["./dist/memo_wasm.js"].exports["__wbindgen_throw"](p0i32,p1i32);
/******/ 					},
/******/ 					"__wbindgen_json_serialize": function(p0i32,p1i32) {
/******/ 						return installedModules["./dist/memo_wasm.js"].exports["__wbindgen_json_serialize"](p0i32,p1i32);
/******/ 					},
/******/ 					"__wbindgen_json_parse": function(p0i32,p1i32) {
/******/ 						return installedModules["./dist/memo_wasm.js"].exports["__wbindgen_json_parse"](p0i32,p1i32);
/******/ 					},
/******/ 					"__wbindgen_object_drop_ref": function(p0i32) {
/******/ 						return installedModules["./dist/memo_wasm.js"].exports["__wbindgen_object_drop_ref"](p0i32);
/******/ 					}
/******/ 				}
/******/ 			};
/******/ 		},
/******/ 	};
/******/
/******/ 	// The require function
/******/ 	function __webpack_require__(moduleId) {
/******/
/******/ 		// Check if module is in cache
/******/ 		if(installedModules[moduleId]) {
/******/ 			return installedModules[moduleId].exports;
/******/ 		}
/******/ 		// Create a new module (and put it into the cache)
/******/ 		var module = installedModules[moduleId] = {
/******/ 			i: moduleId,
/******/ 			l: false,
/******/ 			exports: {}
/******/ 		};
/******/
/******/ 		// Execute the module function
/******/ 		modules[moduleId].call(module.exports, module, module.exports, __webpack_require__);
/******/
/******/ 		// Flag the module as loaded
/******/ 		module.l = true;
/******/
/******/ 		// Return the exports of the module
/******/ 		return module.exports;
/******/ 	}
/******/
/******/ 	// This file contains only the entry chunk.
/******/ 	// The chunk loading function for additional chunks
/******/ 	__webpack_require__.e = function requireEnsure(chunkId) {
/******/ 		var promises = [];
/******/
/******/
/******/ 		// require() chunk loading for javascript
/******/
/******/ 		// "0" is the signal for "already loaded"
/******/ 		if(installedChunks[chunkId] !== 0) {
/******/ 			var chunk = require("./" + chunkId + ".index.js");
/******/ 			var moreModules = chunk.modules, chunkIds = chunk.ids;
/******/ 			for(var moduleId in moreModules) {
/******/ 				modules[moduleId] = moreModules[moduleId];
/******/ 			}
/******/ 			for(var i = 0; i < chunkIds.length; i++)
/******/ 				installedChunks[chunkIds[i]] = 0;
/******/ 		}
/******/
/******/ 		// Fetch + compile chunk loading for webassembly
/******/
/******/ 		var wasmModules = {"0":["./dist/memo_wasm_bg.wasm"]}[chunkId] || [];
/******/
/******/ 		wasmModules.forEach(function(wasmModuleId) {
/******/ 			var installedWasmModuleData = installedWasmModules[wasmModuleId];
/******/
/******/ 			// a Promise means "currently loading" or "already loaded".
/******/ 			if(installedWasmModuleData)
/******/ 				promises.push(installedWasmModuleData);
/******/ 			else {
/******/ 				var importObject = wasmImportObjects[wasmModuleId]();
/******/ 				var req = new Promise(function (resolve, reject) {
/******/ 					var { readFile } = require('fs');
/******/ 					var { join } = require('path');
/******/
/******/ 					try {
/******/ 						readFile(join(__dirname, "" + {"./dist/memo_wasm_bg.wasm":"04b973392e9496048e48"}[wasmModuleId] + ".module.wasm"), function(err, buffer){
/******/ 							if (err) return reject(err);
/******/
/******/ 							// Fake fetch response
/******/ 							resolve({
/******/ 								arrayBuffer() { return Promise.resolve(buffer); }
/******/ 							});
/******/ 						});
/******/ 					} catch (err) { reject(err); }
/******/ 				});
/******/ 				var promise;
/******/ 				if(importObject instanceof Promise) {
/******/ 					var bytesPromise = req.then(function(x) { return x.arrayBuffer(); });
/******/ 					promise = Promise.all([
/******/ 						bytesPromise.then(function(bytes) { return WebAssembly.compile(bytes); }),
/******/ 						importObject
/******/ 					]).then(function(items) {
/******/ 						return WebAssembly.instantiate(items[0], items[1]);
/******/ 					});
/******/ 				} else {
/******/ 					var bytesPromise = req.then(function(x) { return x.arrayBuffer(); });
/******/ 					promise = bytesPromise.then(function(bytes) {
/******/ 						return WebAssembly.instantiate(bytes, importObject);
/******/ 					});
/******/ 				}
/******/ 				promises.push(installedWasmModules[wasmModuleId] = promise.then(function(res) {
/******/ 					return __webpack_require__.w[wasmModuleId] = (res.instance || res).exports;
/******/ 				}));
/******/ 			}
/******/ 		});
/******/ 		return Promise.all(promises);
/******/ 	};
/******/
/******/ 	// expose the modules object (__webpack_modules__)
/******/ 	__webpack_require__.m = modules;
/******/
/******/ 	// expose the module cache
/******/ 	__webpack_require__.c = installedModules;
/******/
/******/ 	// define getter function for harmony exports
/******/ 	__webpack_require__.d = function(exports, name, getter) {
/******/ 		if(!__webpack_require__.o(exports, name)) {
/******/ 			Object.defineProperty(exports, name, { enumerable: true, get: getter });
/******/ 		}
/******/ 	};
/******/
/******/ 	// define __esModule on exports
/******/ 	__webpack_require__.r = function(exports) {
/******/ 		if(typeof Symbol !== 'undefined' && Symbol.toStringTag) {
/******/ 			Object.defineProperty(exports, Symbol.toStringTag, { value: 'Module' });
/******/ 		}
/******/ 		Object.defineProperty(exports, '__esModule', { value: true });
/******/ 	};
/******/
/******/ 	// create a fake namespace object
/******/ 	// mode & 1: value is a module id, require it
/******/ 	// mode & 2: merge all properties of value into the ns
/******/ 	// mode & 4: return value when already ns object
/******/ 	// mode & 8|1: behave like require
/******/ 	__webpack_require__.t = function(value, mode) {
/******/ 		if(mode & 1) value = __webpack_require__(value);
/******/ 		if(mode & 8) return value;
/******/ 		if((mode & 4) && typeof value === 'object' && value && value.__esModule) return value;
/******/ 		var ns = Object.create(null);
/******/ 		__webpack_require__.r(ns);
/******/ 		Object.defineProperty(ns, 'default', { enumerable: true, value: value });
/******/ 		if(mode & 2 && typeof value != 'string') for(var key in value) __webpack_require__.d(ns, key, function(key) { return value[key]; }.bind(null, key));
/******/ 		return ns;
/******/ 	};
/******/
/******/ 	// getDefaultExport function for compatibility with non-harmony modules
/******/ 	__webpack_require__.n = function(module) {
/******/ 		var getter = module && module.__esModule ?
/******/ 			function getDefault() { return module['default']; } :
/******/ 			function getModuleExports() { return module; };
/******/ 		__webpack_require__.d(getter, 'a', getter);
/******/ 		return getter;
/******/ 	};
/******/
/******/ 	// Object.prototype.hasOwnProperty.call
/******/ 	__webpack_require__.o = function(object, property) { return Object.prototype.hasOwnProperty.call(object, property); };
/******/
/******/ 	// __webpack_public_path__
/******/ 	__webpack_require__.p = "";
/******/
/******/ 	// uncaught error handler for webpack runtime
/******/ 	__webpack_require__.oe = function(err) {
/******/ 		process.nextTick(function() {
/******/ 			throw err; // catch this error by using import().catch()
/******/ 		});
/******/ 	};
/******/
/******/ 	// object with all WebAssembly.instance exports
/******/ 	__webpack_require__.w = {};
/******/
/******/
/******/ 	// Load entry module and return exports
/******/ 	return __webpack_require__(__webpack_require__.s = "./src/index.ts");
/******/ })
/************************************************************************/
/******/ ({

/***/ "./src/index.ts":
/*!**********************!*\
  !*** ./src/index.ts ***!
  \**********************/
/*! exports provided: init */
/***/ (function(module, __webpack_exports__, __webpack_require__) {

"use strict";
eval("__webpack_require__.r(__webpack_exports__);\n/* harmony export (binding) */ __webpack_require__.d(__webpack_exports__, \"init\", function() { return init; });\nvar __awaiter = (undefined && undefined.__awaiter) || function (thisArg, _arguments, P, generator) {\n    return new (P || (P = Promise))(function (resolve, reject) {\n        function fulfilled(value) { try { step(generator.next(value)); } catch (e) { reject(e); } }\n        function rejected(value) { try { step(generator[\"throw\"](value)); } catch (e) { reject(e); } }\n        function step(result) { result.done ? resolve(result.value) : new P(function (resolve) { resolve(result.value); }).then(fulfilled, rejected); }\n        step((generator = generator.apply(thisArg, _arguments || [])).next());\n    });\n};\nvar __generator = (undefined && undefined.__generator) || function (thisArg, body) {\n    var _ = { label: 0, sent: function() { if (t[0] & 1) throw t[1]; return t[1]; }, trys: [], ops: [] }, f, y, t, g;\n    return g = { next: verb(0), \"throw\": verb(1), \"return\": verb(2) }, typeof Symbol === \"function\" && (g[Symbol.iterator] = function() { return this; }), g;\n    function verb(n) { return function (v) { return step([n, v]); }; }\n    function step(op) {\n        if (f) throw new TypeError(\"Generator is already executing.\");\n        while (_) try {\n            if (f = 1, y && (t = op[0] & 2 ? y[\"return\"] : op[0] ? y[\"throw\"] || ((t = y[\"return\"]) && t.call(y), 0) : y.next) && !(t = t.call(y, op[1])).done) return t;\n            if (y = 0, t) op = [op[0] & 2, t.value];\n            switch (op[0]) {\n                case 0: case 1: t = op; break;\n                case 4: _.label++; return { value: op[1], done: false };\n                case 5: _.label++; y = op[1]; op = [0]; continue;\n                case 7: op = _.ops.pop(); _.trys.pop(); continue;\n                default:\n                    if (!(t = _.trys, t = t.length > 0 && t[t.length - 1]) && (op[0] === 6 || op[0] === 2)) { _ = 0; continue; }\n                    if (op[0] === 3 && (!t || (op[1] > t[0] && op[1] < t[3]))) { _.label = op[1]; break; }\n                    if (op[0] === 6 && _.label < t[1]) { _.label = t[1]; t = op; break; }\n                    if (t && _.label < t[2]) { _.label = t[2]; _.ops.push(op); break; }\n                    if (t[2]) _.ops.pop();\n                    _.trys.pop(); continue;\n            }\n            op = body.call(thisArg, _);\n        } catch (e) { op = [6, e]; y = 0; } finally { f = t = 0; }\n        if (op[0] & 5) throw op[1]; return { value: op[0] ? op[1] : void 0, done: true };\n    }\n};\nvar server;\nfunction init() {\n    return __awaiter(this, void 0, void 0, function () {\n        var memo;\n        return __generator(this, function (_a) {\n            switch (_a.label) {\n                case 0: return [4 /*yield*/, __webpack_require__.e(/*! import() */ 0).then(__webpack_require__.bind(null, /*! ../dist/memo_wasm */ \"./dist/memo_wasm.js\"))];\n                case 1:\n                    memo = _a.sent();\n                    if (!server) {\n                        server = memo.Server[\"new\"]();\n                    }\n                    return [2 /*return*/, { WorkTree: WorkTree }];\n            }\n        });\n    });\n}\nfunction request(req) {\n    var response = server.request(req);\n    if (response.type == \"Error\") {\n        throw new Error(response.message);\n    }\n    else {\n        return response;\n    }\n}\nvar FileType;\n(function (FileType) {\n    FileType[\"Directory\"] = \"Directory\";\n    FileType[\"File\"] = \"File\";\n})(FileType || (FileType = {}));\nvar FileStatus;\n(function (FileStatus) {\n    FileStatus[\"New\"] = \"New\";\n    FileStatus[\"Renamed\"] = \"Renamed\";\n    FileStatus[\"Removed\"] = \"Removed\";\n    FileStatus[\"Modified\"] = \"Modified\";\n    FileStatus[\"Unchanged\"] = \"Unchanged\";\n})(FileStatus || (FileStatus = {}));\nvar WorkTree = /** @class */ (function () {\n    function WorkTree(replicaId) {\n        this.id = request({\n            type: \"CreateWorkTree\",\n            replica_id: replicaId\n        }).tree_id;\n    }\n    WorkTree.getRootFileId = function () {\n        if (!WorkTree.rootFileId) {\n            WorkTree.rootFileId = request({ type: \"GetRootFileId\" }).file_id;\n        }\n        return WorkTree.rootFileId;\n    };\n    WorkTree.prototype.getVersion = function () {\n        return request({ tree_id: this.id, type: \"GetVersion\" }).version;\n    };\n    WorkTree.prototype.appendBaseEntries = function (baseEntries) {\n        return request({\n            type: \"AppendBaseEntries\",\n            tree_id: this.id,\n            entries: baseEntries\n        }).operations;\n    };\n    WorkTree.prototype.applyOps = function (operations) {\n        var response = request({\n            type: \"ApplyOperations\",\n            tree_id: this.id,\n            operations: operations\n        });\n        return response.operations;\n    };\n    WorkTree.prototype.newTextFile = function () {\n        var _a = request({\n            type: \"NewTextFile\",\n            tree_id: this.id\n        }), file_id = _a.file_id, operation = _a.operation;\n        return { fileId: file_id, operation: operation };\n    };\n    WorkTree.prototype.createDirectory = function (parentId, name) {\n        var _a = request({\n            type: \"CreateDirectory\",\n            tree_id: this.id,\n            parent_id: parentId,\n            name: name\n        }), file_id = _a.file_id, operation = _a.operation;\n        return { fileId: file_id, operation: operation };\n    };\n    WorkTree.prototype.openTextFile = function (fileId, baseText) {\n        var response = request({\n            type: \"OpenTextFile\",\n            tree_id: this.id,\n            file_id: fileId,\n            base_text: baseText\n        });\n        return response.buffer_id;\n    };\n    WorkTree.prototype.rename = function (fileId, newParentId, newName) {\n        return request({\n            type: \"Rename\",\n            tree_id: this.id,\n            file_id: fileId,\n            new_parent_id: newParentId,\n            new_name: newName\n        }).operation;\n    };\n    WorkTree.prototype.remove = function (fileId) {\n        return request({\n            type: \"Remove\",\n            tree_id: this.id,\n            file_id: fileId\n        }).operation;\n    };\n    WorkTree.prototype.edit = function (bufferId, ranges, newText) {\n        var response = request({\n            type: \"Edit\",\n            tree_id: this.id,\n            buffer_id: bufferId,\n            ranges: ranges,\n            new_text: newText\n        });\n        return response.operation;\n    };\n    WorkTree.prototype.changesSince = function (bufferId, version) {\n        return request({\n            type: \"ChangesSince\",\n            tree_id: this.id,\n            buffer_id: bufferId,\n            version: version\n        }).changes;\n    };\n    WorkTree.prototype.getText = function (bufferId) {\n        return request({\n            type: \"GetText\",\n            tree_id: this.id,\n            buffer_id: bufferId\n        }).text;\n    };\n    WorkTree.prototype.fileIdForPath = function (path) {\n        return request({\n            type: \"FileIdForPath\",\n            tree_id: this.id,\n            path: path\n        }).file_id;\n    };\n    WorkTree.prototype.pathForFileId = function (id) {\n        return request({\n            type: \"PathForFileId\",\n            tree_id: this.id,\n            file_id: id\n        }).path;\n    };\n    WorkTree.prototype.entries = function (options) {\n        var showDeleted, descendInto;\n        if (options) {\n            showDeleted = options.showDeleted || false;\n            descendInto = options.descendInto || null;\n        }\n        else {\n            showDeleted = false;\n            descendInto = null;\n        }\n        return request({\n            type: \"Entries\",\n            tree_id: this.id,\n            show_deleted: showDeleted,\n            descend_into: descendInto\n        }).entries;\n    };\n    return WorkTree;\n}());\n\n\n//# sourceURL=webpack://memo/./src/index.ts?");

/***/ }),

/***/ "util":
/*!***********************!*\
  !*** external "util" ***!
  \***********************/
/*! no static exports found */
/***/ (function(module, exports) {

eval("module.exports = require(\"util\");\n\n//# sourceURL=webpack://memo/external_%22util%22?");

/***/ })

/******/ });
});