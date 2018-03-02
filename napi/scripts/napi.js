#!/usr/bin/env node

const parseArgs = require('minimist')
const cp = require('child_process')
const path = require('path')

const parsedNodeVersion = process.versions.node.match(/^(\d+)\.(\d+)\.(\d+)$/)
const nodeMajorVersion = parseInt(parsedNodeVersion[1])
const nodeMinorVersion = parseInt(parsedNodeVersion[2])

if (nodeMajorVersion < 8 || (nodeMajorVersion === 8 && nodeMinorVersion < 9)) {
  console.error("This build script should be run on Node 8.9 or greater")
  process.exit(1)
}

const argv = parseArgs(process.argv.slice(2), {
  boolean: ['release']
})

const subcommand = argv._[0] || 'build'

const nodeIncludePath = path.join(process.argv[0], '..', '..', 'include', 'node')
const moduleName = path.basename(process.cwd());
process.env.NODE_INCLUDE_PATH = nodeIncludePath

switch (subcommand) {
  case 'build':
    const featuresFlag = `--features node${nodeMajorVersion}`
    const releaseFlag = argv.release ? '--release' : ''
    const targetDir = argv.release ? 'release' : 'debug'
    cp.execSync(`cargo rustc ${featuresFlag} ${releaseFlag} -- -Clink-args=\"-undefined dynamic_lookup -export_dynamic\"`, {stdio: 'inherit'})
    cp.execSync(`cp target/${targetDir}/{lib${moduleName}.dylib,${moduleName}.node}`, {stdio: 'inherit'})
    break;
  case 'check':
    cp.execSync(`cargo check`, {stdio: 'inherit'})
  case 'doc':
    cp.execSync(`cargo doc`, {stdio: 'inherit'})
}
