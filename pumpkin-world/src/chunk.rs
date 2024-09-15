use std::cmp::max;
use std::collections::HashMap;
use std::ops::Index;

use fastnbt::LongArray;
use pumpkin_core::math::vector2::Vector2;
use serde::{Deserialize, Serialize};

use crate::{
    block::BlockId,
    coordinates::{ChunkRelativeBlockCoordinates, Height},
    level::{ChunkNotGeneratedError, WorldError},
    WORLD_HEIGHT,
};

const CHUNK_AREA: usize = 16 * 16;
const SUBCHUNK_VOLUME: usize = CHUNK_AREA * 16;
const CHUNK_VOLUME: usize = CHUNK_AREA * WORLD_HEIGHT;

pub struct ChunkData {
    pub blocks: ChunkBlocks,
    pub position: Vector2<i32>,
}

pub struct ChunkBlocks {
    // TODO make this a Vec that doesn't store the upper layers that only contain air

    // The packet relies on this ordering -> leave it like this for performance
    /// Ordering: yzx (y being the most significant)
    blocks: Box<[BlockId; CHUNK_VOLUME]>,

    /// See `https://minecraft.fandom.com/wiki/Heightmap` for more info
    pub heightmap: ChunkHeightmaps,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "PascalCase")]
struct PaletteEntry {
    name: String,
    properties: Option<HashMap<String, String>>,
}

#[derive(Deserialize, Debug, Clone)]
struct ChunkSectionBlockStates {
    data: Option<LongArray>,
    palette: Vec<PaletteEntry>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "UPPERCASE")]
pub struct ChunkHeightmaps {
    motion_blocking: LongArray,
    world_surface: LongArray,
}

#[derive(Deserialize, Debug)]
#[expect(dead_code)]
struct ChunkSection {
    #[serde(rename = "Y")]
    y: i32,
    block_states: Option<ChunkSectionBlockStates>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "PascalCase")]
struct ChunkNbt {
    #[expect(dead_code)]
    data_version: usize,

    #[serde(rename = "sections")]
    sections: Vec<ChunkSection>,

    heightmaps: ChunkHeightmaps,
}

#[derive(Deserialize, Debug, PartialEq, Eq)]
#[serde(tag = "Status")]
enum ChunkStatus {
    #[serde(rename = "minecraft:empty")]
    Empty,
    #[serde(rename = "minecraft:structure_starts")]
    StructureStarts,
    #[serde(rename = "minecraft:structure_references")]
    StructureReferences,
    #[serde(rename = "minecraft:biomes")]
    Biomes,
    #[serde(rename = "minecraft:noise")]
    Noise,
    #[serde(rename = "minecraft:surface")]
    Surface,
    #[serde(rename = "minecraft:carvers")]
    Carvers,
    #[serde(rename = "minecraft:liquid_carvers")]
    LiquidCarvers,
    #[serde(rename = "minecraft:features")]
    Features,
    #[serde(rename = "minecraft:initialize_light")]
    Light,
    #[serde(rename = "minecraft:spawn")]
    Spawn,
    #[serde(rename = "minecraft:heightmaps")]
    Heightmaps,
    #[serde(rename = "minecraft:full")]
    Full,
}

/// The Heightmap for a completely empty chunk
impl Default for ChunkHeightmaps {
    fn default() -> Self {
        Self {
            // 0 packed into an i64 7 times.
            motion_blocking: LongArray::new(vec![0; 37]),
            world_surface: LongArray::new(vec![0; 37]),
        }
    }
}

impl Default for ChunkBlocks {
    fn default() -> Self {
        Self {
            blocks: Box::new([BlockId::default(); CHUNK_VOLUME]),
            heightmap: ChunkHeightmaps::default(),
        }
    }
}

impl ChunkBlocks {
    pub fn empty_with_heightmap(heightmap: ChunkHeightmaps) -> Self {
        Self {
            blocks: Box::new([BlockId::default(); CHUNK_VOLUME]),
            heightmap,
        }
    }

    /// Gets the given block in the chunk
    pub fn get_block(&self, position: ChunkRelativeBlockCoordinates) -> BlockId {
        self.blocks[Self::convert_index(position)]
    }

    /// Sets the given block in the chunk, returning the old block
    pub fn set_block(
        &mut self,
        position: ChunkRelativeBlockCoordinates,
        block: BlockId,
    ) -> BlockId {
        // TODO @LUK_ESC? update the heightmap
        self.set_block_no_heightmap_update(position, block)
    }

    /// Sets the given block in the chunk, returning the old block
    /// Contrary to `set_block` this does not update the heightmap.
    ///
    /// Only use this if you know you don't need to update the heightmap
    /// or if you manually set the heightmap in `empty_with_heightmap`
    pub fn set_block_no_heightmap_update(
        &mut self,
        position: ChunkRelativeBlockCoordinates,
        block: BlockId,
    ) -> BlockId {
        std::mem::replace(&mut self.blocks[Self::convert_index(position)], block)
    }

    pub fn iter_subchunks(&self) -> impl Iterator<Item = &[BlockId; SUBCHUNK_VOLUME]> {
        self.blocks
            .chunks(SUBCHUNK_VOLUME)
            .map(|subchunk| subchunk.try_into().unwrap())
    }

    fn convert_index(index: ChunkRelativeBlockCoordinates) -> usize {
        // % works for negative numbers as intended.
        index.y.get_absolute() as usize * CHUNK_AREA + *index.z as usize * 16 + *index.x as usize
    }

    #[expect(dead_code)]
    fn calculate_heightmap(&self) -> ChunkHeightmaps {
        // figure out how LongArray is formatted
        // figure out how to find out if block is motion blocking
        todo!()
    }
}

impl Index<ChunkRelativeBlockCoordinates> for ChunkBlocks {
    type Output = BlockId;

    fn index(&self, index: ChunkRelativeBlockCoordinates) -> &Self::Output {
        &self.blocks[Self::convert_index(index)]
    }
}

impl ChunkData {
    pub fn from_bytes(chunk_data: Vec<u8>, at: Vector2<i32>) -> Result<Self, WorldError> {
        if fastnbt::from_bytes::<ChunkStatus>(&chunk_data).expect("Failed reading chunk status.")
            != ChunkStatus::Full
        {
            return Err(WorldError::ChunkNotGenerated(
                ChunkNotGeneratedError::IncompleteGeneration,
            ));
        }

        let chunk_data = match fastnbt::from_bytes::<ChunkNbt>(chunk_data.as_slice()) {
            Ok(v) => v,
            Err(err) => return Err(WorldError::ErrorDeserializingChunk(err.to_string())),
        };

        // this needs to be boxed, otherwise it will cause a stack-overflow
        let mut blocks = ChunkBlocks::empty_with_heightmap(chunk_data.heightmaps);
        let mut block_index = 0; // which block we're currently at

        for section in chunk_data.sections.into_iter() {
            let block_states = match section.block_states {
                Some(states) => states,
                None => continue, // TODO @lukas0008 this should instead fill all blocks with the only element of the palette
            };

            let palette = block_states
                .palette
                .iter()
                .map(|entry| BlockId::new(&entry.name, entry.properties.as_ref()))
                .collect::<Result<Vec<_>, _>>()?;

            let block_data = match block_states.data {
                None => {
                    // We skipped placing an empty subchunk.
                    // We need to increase the y coordinate of the next subchunk being placed.
                    block_index += SUBCHUNK_VOLUME;
                    continue;
                }
                Some(d) => d,
            }
            .into_inner();

            // How many bits each block has in one of the pallete u64s
            let block_bit_size = {
                let size = 64 - (palette.len() as i64 - 1).leading_zeros();
                max(4, size)
            };
            // How many blocks there are in one of the palletes u64s
            let blocks_in_pallete = 64 / block_bit_size;

            let mask = (1 << block_bit_size) - 1;
            'block_loop: for block in block_data.iter() {
                for i in 0..blocks_in_pallete {
                    let index = (block >> (i * block_bit_size)) & mask;
                    let block = palette[index as usize];

                    // TODO allow indexing blocks directly so we can just use block_index and save some time?
                    // this is fine because we initalized the heightmap of `blocks`
                    // from the cached value in the world file
                    blocks.set_block_no_heightmap_update(
                        ChunkRelativeBlockCoordinates {
                            z: ((block_index % CHUNK_AREA) / 16).into(),
                            y: Height::from_absolute((block_index / CHUNK_AREA) as u16),
                            x: (block_index % 16).into(),
                        },
                        block,
                    );

                    block_index += 1;

                    // if `SUBCHUNK_VOLUME `is not divisible by `blocks_in_pallete` the block_data
                    // can sometimes spill into other subchunks. We avoid that by aborting early
                    if (block_index % SUBCHUNK_VOLUME) == 0 {
                        break 'block_loop;
                    }
                }
            }
        }

        Ok(ChunkData {
            blocks,
            position: at,
        })
    }
}

mod serialization {
    use fastnbt::LongArray;
    use pumpkin_core::math::vector2::Vector2;
    use speedy::{LittleEndian, Readable, Writable};

    use crate::{block::BlockId, chunk::CHUNK_VOLUME};

    use super::{ChunkBlocks, ChunkData, ChunkHeightmaps};

    impl Writable<LittleEndian> for ChunkData {
        fn write_to< T: ?Sized + speedy::Writer< LittleEndian > >( &self, writer: &mut T ) -> Result< (), <LittleEndian as speedy::Context>::Error > {
            
            // Write X and Z chunk coordinate
            writer.write_i32(self.position.x)?;
            writer.write_i32(self.position.z)?;
            
            // BlocksId
            for block in self.blocks.blocks.iter() {
                writer.write_u16(block.get_id())?
            }
            
            // Heightmap (motion then world surface)
            writer.write_u64(self.blocks.heightmap.motion_blocking.len() as _)?;
            for motion in self.blocks.heightmap.motion_blocking.iter() {
                writer.write_i64(*motion)?
            }
            writer.write_u64(self.blocks.heightmap.world_surface.len() as _)?;
            for surface in self.blocks.heightmap.world_surface.iter() {
                writer.write_i64(*surface)?
            }
            
            Ok(())
        }
    }
    
    impl<'t> Readable<'t, LittleEndian> for ChunkData {
        fn read_from< R: speedy::Reader< 't, LittleEndian > >( reader: &mut R ) -> Result< Self, <LittleEndian as speedy::Context>::Error > {
            
            // Read X and Z chunk coordinate
            let position = Vector2 { x: reader.read_i32()?, z: reader.read_i32()? };
            
            // BlocksId
            let mut blocks = Vec::with_capacity(CHUNK_VOLUME);
            for _ in 0..CHUNK_VOLUME {
                blocks.push(BlockId::from_id(reader.read_u16()?));
            }
            
            // Heightmap (motion then world surface) 
            let len = reader.read_u64()? as usize;
            let mut motion = Vec::with_capacity(len);
            for _ in 0..CHUNK_VOLUME {
                motion.push(reader.read_i64()?);
            }
            let len = reader.read_u64()? as usize;
            let mut surface = Vec::with_capacity(len);
            for _ in 0..CHUNK_VOLUME {
                surface.push(reader.read_i64()?);
            }
            
            Ok(
                ChunkData { 
                    blocks: ChunkBlocks {
                        blocks: blocks.into_boxed_slice().try_into().map_err(|_| speedy::Error::custom("Block count isn't the volume of a chunk!"))?,
                        heightmap: ChunkHeightmaps {
                            motion_blocking: LongArray::new(motion), 
                            world_surface: LongArray::new(surface)
                        }
                    }, 
                    position 
                }
            )
        }
    }
}
