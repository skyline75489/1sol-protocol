import {
  createOneSolProtocol,
  prepareTokenSwap,
  swap,
} from './swap-test';

export async function main() {
  // These test cases are designed to run sequentially and in the following order
  console.log('Run test: prepareTokenSwap');
  await prepareTokenSwap();
  console.log('Run test: createOneSolProtocol');
  await createOneSolProtocol();
  
  //These test cases are designed to run sequentially and in the following order
  console.log('Run test: swap');
  await swap();
  
  console.log('Success\n');
}
  
main()
  .catch(err => {
    console.error(err);
    process.exit(-1);
  })
  .then(() => process.exit());