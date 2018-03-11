const { exec } = require('child_process');
const { existsSync, statSync, readFileSync, readdirSync } = require('fs');
const { sep, join, dirname } = require('path');

// Added this module ignore to exclude test_module wich currenly throws
// an error because it has to be npm-insatlled first
const ignore = /node_modules|module/; 
const fields = ['dependencies', 'devDependencies', 'peerDependencies']

const relativify = str => str.replace(process.cwd() + '/', '')

function buildTree(basedir) {
  const files = readdirSync(basedir);
  let results = {};

  files.forEach(file => {
    const filepath = relativify(join(basedir, file));

    if(ignore.test(filepath)) return;

    if(file === 'package.json') {
      results[filepath] = readFileSync(filepath).toString();
    } else if(statSync(filepath).isDirectory()) {
      const res = buildTree(filepath);

      results = Object.assign({}, results, res);
    }
  })

  return results
}

function linkDependencies(packages) {
  const res = {}
  
  Object.keys(packages).forEach(package => {
    const data = JSON.parse(packages[package]);

    fields.forEach(field => {
      if(data[field]) {
        const deps = data[field];

        Object.keys(deps) // All the dependencies under this field(dev, peer, etc)
          .filter(key => deps[key].charAt(0) == '.') // Filter only these that does not have a semantic version
          .forEach(key => {
            const dir = dirname(package);
            const pkg = join(dir, deps[key]);

            if(!res[pkg]) res[pkg] = [];

            res[pkg].push(dir);
          })
      }
    })
  })

  return res;
}

exec('git --no-pager diff --name-only origin HEAD', function(err, stdout, stderr) {
  if(err) {
    console.error('Error while diffing the commits', err);

    console.error('Full stderr:', stderr, '\n\n');

    process.exit(1);
  }

  console.log('Raw changes: \n' + stdout)

  const packages = buildTree(process.cwd());
  const diffs    = stdout
    .split('\n') // Every new line is a file changed
    .filter(Boolean) // Remove empty strings
    .map(path => path.split(sep)[0]) // /a/b/c -> a
    .filter((item, pos, self) => self.indexOf(item) == pos) // Remove duplicates

  console.log(`File changed from the origin branch:  
  ${diffs.join('\n  ')}
    `);

  console.log(`Found these package.jsons(s): 
  ${Object.keys(packages).join('\n  ')}
    `);

  const linkedState = linkDependencies(packages)

  diffs
    .map(mod => {
      // For example if napi changes it triggers 
      // test for: napi(itself), xray_node and napi/test_module

      if (linkedState[mod]) {
        console.log('Changes found in', mod + ', testing it and its dependents')

        return [mod].concat(linkedState[mod])
      }
      else return null
    })
    .filter(Boolean)
    .forEach(modulesToTest => modulesToTest.forEach(runTest))
})

function runTest(module) {
  console.log('Testing module', module)

  exec('npm run-script check', { cwd: join(process.cwd(), module) }, (err, stdout, stderr) => {
    if(err) {

      console.log('\n\n Error while testing module', module, '\n\n')

      console.error(err.message)

    } else {
      console.log(stdout)
    }
  })
}